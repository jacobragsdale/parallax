/**
 * The protocol client, independent of Angular.
 *
 * One connection carries every view over one transport, a WebSocket to the server
 * or a worker hosting the WebAssembly engine. Two rules keep stale data off the
 * screen and the engine from being flooded:
 *
 * - A view drops any frame built from an older configuration than the one it last
 *   sent, or from an older table load than one it already showed.
 * - At most one viewport request per view is in flight, matched to its reply by
 *   request id. Scrolling faster than frames arrive replaces the pending request
 *   instead of queueing them.
 *
 * A one-off window, such as the cells a user copies, is fetched without moving the
 * viewport; its reply is matched by request id and never drawn.
 */

import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import type { MessageInitShape } from "@bufbuild/protobuf";

import { decodeFrame, FrameError } from "./frame";
import type { FrameData } from "./frame";
import { ClientMessageSchema, ServerMessageSchema } from "./proto/parallax_pb";
import type { TableStatus, ViewOpened, Welcome } from "./proto/parallax_pb";

/** The body of a client message, as plain values. */
export type ClientBody = NonNullable<MessageInitShape<typeof ClientMessageSchema>["body"]>;

/** Moves encoded messages between the client and an engine. */
export interface Transport {
  readonly send: (bytes: Uint8Array) => void;
  readonly close: () => void;
}

/** What a view reports back. */
export interface ViewListener {
  readonly frame: (frame: FrameData) => void;
  readonly opened?: (opened: ViewOpened) => void;
  readonly status?: (status: TableStatus) => void;
  readonly error?: (message: string) => void;
}

/** A window of rows and data columns. */
export interface Viewport {
  readonly firstRow: number;
  readonly rowCount: number;
  readonly firstColumn: number;
  readonly columnCount: number;
}

function sameViewport(a: Viewport | null, b: Viewport): boolean {
  return a !== null && a.firstRow === b.firstRow && a.rowCount === b.rowCount && a.firstColumn === b.firstColumn && a.columnCount === b.columnCount;
}

/** One open view. */
export class ViewHandle {
  private configVersion = 0n;
  private lastLoadVersion = 0n;
  private inFlight: { readonly viewport: Viewport; readonly requestId: bigint } | null = null;
  private pending: Viewport | null = null;
  private closed = false;
  private readonly fetches = new Map<bigint, { readonly resolve: (frame: FrameData) => void; readonly reject: (error: Error) => void }>();

  constructor(
    public readonly id: number,
    private readonly connection: Connection,
    private readonly listener: ViewListener
  ) {}

  /** The most recent window asked for, whether answered yet or not. */
  public requested(): Viewport | null {
    return this.pending ?? this.inFlight?.viewport ?? null;
  }

  /** Asks for a window of rows and data columns. */
  public setViewport(viewport: Viewport): void {
    if (this.closed || sameViewport(this.inFlight?.viewport ?? null, viewport) || sameViewport(this.pending, viewport)) {
      return;
    }
    if (this.inFlight !== null) {
      this.pending = viewport;
      return;
    }
    this.sendViewport(viewport);
  }

  /** Fetches one window without moving the viewport, for copying. At most 1,000,000 cells. */
  public fetch(viewport: Viewport): Promise<FrameData> {
    if (this.closed) {
      return Promise.reject(new Error("the view is closed"));
    }
    return new Promise((resolve, reject) => {
      const requestId = this.connection.send({
        case: "fetchWindow",
        value: { viewId: this.id, firstRow: BigInt(viewport.firstRow), rowCount: BigInt(viewport.rowCount), firstColumn: viewport.firstColumn, columnCount: viewport.columnCount }
      });
      this.fetches.set(requestId, { resolve, reject });
    });
  }

  /** Replaces the view's layout. Frames from the old layout are dropped from now on. */
  public update(layoutJson: string): void {
    if (this.closed) {
      return;
    }
    this.configVersion += 1n;
    this.connection.send({ case: "updateView", value: { viewId: this.id, layoutJson, configVersion: this.configVersion } });
  }

  /** Expands or collapses the group at `path`. */
  public setExpanded(path: readonly (string | number | boolean | null)[], expanded: boolean): void {
    if (!this.closed) {
      this.connection.send({ case: "setExpanded", value: { viewId: this.id, pathJson: JSON.stringify(path), expanded } });
    }
  }

  /** `true` once the view is closed. */
  public get isClosed(): boolean {
    return this.closed;
  }

  /** Closes the view. Fetches still waiting fail. */
  public close(): void {
    if (!this.closed) {
      this.closed = true;
      for (const pending of this.fetches.values()) {
        pending.reject(new Error("the view is closed"));
      }
      this.fetches.clear();
      this.connection.send({ case: "closeView", value: { viewId: this.id } });
      this.connection.forget(this.id);
    }
  }

  private sendViewport(viewport: Viewport): void {
    const requestId = this.connection.send({
      case: "setViewport",
      value: { viewId: this.id, firstRow: BigInt(viewport.firstRow), rowCount: BigInt(viewport.rowCount), firstColumn: viewport.firstColumn, columnCount: viewport.columnCount }
    });
    this.inFlight = { viewport, requestId };
  }

  /** Releases the in-flight viewport if `requestId` answers it, and sends any pending one. */
  private settle(requestId: bigint): void {
    if (this.inFlight?.requestId !== requestId) {
      return;
    }
    this.inFlight = null;
    const next = this.pending;
    this.pending = null;
    if (next !== null) {
      this.sendViewport(next);
    }
  }

  /** @internal Called by the connection. */
  public receiveFrame(frame: FrameData, requestId: bigint): void {
    const fetch = this.fetches.get(requestId);
    if (fetch !== undefined) {
      this.fetches.delete(requestId);
      fetch.resolve(frame);
      return;
    }
    this.settle(requestId);
    if (frame.configVersion < this.configVersion || frame.loadVersion < this.lastLoadVersion) {
      return;
    }
    this.lastLoadVersion = frame.loadVersion;
    this.listener.frame(frame);
  }

  /** @internal Called by the connection when the transport closes: fetches fail and the view reports why. */
  public receiveDisconnect(reason: string): void {
    for (const pending of this.fetches.values()) {
      pending.reject(new Error(reason));
    }
    this.fetches.clear();
    this.inFlight = null;
    this.pending = null;
    this.closed = true;
    this.listener.error?.(reason);
  }

  /** @internal Called by the connection. */
  public receiveOpened(opened: ViewOpened): void {
    this.listener.opened?.(opened);
  }

  /** @internal Called by the connection. */
  public receiveStatus(status: TableStatus): void {
    this.listener.status?.(status);
  }

  /** @internal Called by the connection. */
  public receiveError(message: string, requestId: bigint): void {
    const fetch = this.fetches.get(requestId);
    if (fetch !== undefined) {
      this.fetches.delete(requestId);
      fetch.reject(new Error(message));
      return;
    }
    this.settle(requestId);
    this.listener.error?.(message);
  }
}

/** A protocol connection over one transport. */
export class Connection {
  private nextRequest = 1n;
  private nextView = 1;
  private readonly views = new Map<number, ViewHandle>();
  private welcome: Welcome | null = null;
  private readonly errors: ((message: string) => void)[] = [];

  constructor(private readonly transport: Transport) {
    this.send({ case: "hello", value: { protocolVersion: 1 } });
  }

  /** Connects to a server's `/ws` endpoint. */
  public static websocket(url: string): Promise<Connection> {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(url);
      socket.binaryType = "arraybuffer";
      let connection: Connection | null = null;
      socket.addEventListener("open", () => {
        connection = new Connection({
          send: (bytes) => {
            socket.send(new Uint8Array(bytes));
          },
          close: () => {
            socket.close();
          }
        });
        resolve(connection);
      });
      socket.addEventListener("message", (event: MessageEvent<unknown>) => {
        const data = event.data;
        if (connection !== null && data instanceof ArrayBuffer) {
          connection.receive(new Uint8Array(data));
        }
      });
      socket.addEventListener("error", () => {
        reject(new Error(`could not connect to ${url}`));
      });
      socket.addEventListener("close", () => {
        connection?.disconnect("The connection to the server closed. Reload the page to reconnect.");
      });
    });
  }

  /** The datasets the engine offers, once it has said hello. */
  public get datasets(): readonly string[] {
    return this.welcome?.datasets.map((d) => d.name) ?? [];
  }

  /** Reports errors not tied to a view. */
  public onError(listener: (message: string) => void): void {
    this.errors.push(listener);
  }

  /** Opens a view of `dataset` with a layout document. */
  public open(dataset: string, layoutJson: string, listener: ViewListener, parameters: Readonly<Record<string, string>> = {}): ViewHandle {
    const id = this.nextView;
    this.nextView += 1;
    const view = new ViewHandle(id, this, listener);
    this.views.set(id, view);
    this.send({ case: "openView", value: { viewId: id, dataset, layoutJson, parameters: { ...parameters } } });
    return view;
  }

  /** Closes the transport. */
  public close(): void {
    this.transport.close();
  }

  /** @internal Encodes and sends a message, returning its request id. */
  public send(body: ClientBody): bigint {
    const requestId = this.nextRequest;
    this.nextRequest += 1n;
    this.transport.send(toBinary(ClientMessageSchema, create(ClientMessageSchema, { requestId, body })));
    return requestId;
  }

  /** Marks every view disconnected: pending fetches fail and each view reports `reason`. */
  public disconnect(reason: string): void {
    for (const view of [...this.views.values()]) {
      view.receiveDisconnect(reason);
    }
    this.views.clear();
    for (const listener of this.errors) {
      listener(reason);
    }
  }

  /** @internal Forgets a closed view. */
  public forget(id: number): void {
    this.views.delete(id);
  }

  /** Handles one encoded server message. */
  public receive(bytes: Uint8Array): void {
    const message = fromBinary(ServerMessageSchema, bytes);
    const requestId = message.requestId;
    const body = message.body;
    switch (body.case) {
      case "welcome":
        this.welcome = body.value;
        return;
      case "viewOpened":
        this.views.get(body.value.viewId)?.receiveOpened(body.value);
        return;
      case "frame": {
        const view = this.views.get(body.value.viewId);
        if (view === undefined) {
          return;
        }
        let frame: FrameData;
        try {
          frame = decodeFrame(body.value);
        } catch (error: unknown) {
          view.receiveError(error instanceof FrameError ? error.message : String(error), requestId);
          return;
        }
        view.receiveFrame(frame, requestId);
        return;
      }
      case "tableStatus":
        this.views.get(body.value.viewId)?.receiveStatus(body.value);
        return;
      case "error": {
        const view = this.views.get(body.value.viewId);
        if (view === undefined) {
          for (const listener of this.errors) {
            listener(body.value.message);
          }
        } else {
          view.receiveError(body.value.message, requestId);
        }
        return;
      }
      case "distinctValues":
      case undefined:
        return;
    }
  }
}
