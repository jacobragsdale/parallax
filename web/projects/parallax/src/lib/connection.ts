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

import { decodeChartFrame } from "./chart-frame";
import type { ChartFrameData } from "./chart-frame";
import { decodeDistinct, decodeFrame, FrameError } from "./frame";
import type { DistinctValue, FrameData } from "./frame";
import { ClientMessageSchema, ServerMessageSchema } from "./proto/parallax_pb";
import type { ChartFrame, DatasetInfo, DistinctValuesResult, Frame, TableStatus, ViewOpened, Welcome } from "./proto/parallax_pb";

/** The body of a client message, as plain values. */
export type ClientBody = NonNullable<MessageInitShape<typeof ClientMessageSchema>["body"]>;

/** Where a connection's engine runs. */
export type EngineLocation = "server" | "browser";

/** Moves encoded messages between the client and an engine. */
export interface Transport {
  readonly send: (bytes: Uint8Array) => void;
  readonly close: () => void;
}

/** What a view reports back. */
export interface ViewListener {
  readonly frame: (frame: FrameData) => void;
  /** A chart's reduction, for views whose layout has a chart the engine reduces. */
  readonly chart?: (frame: ChartFrameData) => void;
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

/** The part of a chart to reduce and the plot it fills, in pixels. Without an x range, the whole data. */
export interface ChartWindow {
  readonly x?: readonly [number, number];
  /** The y range of a density; without it, the data's. */
  readonly y?: readonly [number, number];
  readonly width: number;
  readonly height: number;
}

function sameChartWindow(a: ChartWindow | null, b: ChartWindow): boolean {
  return a !== null && a.width === b.width && a.height === b.height && a.x?.[0] === b.x?.[0] && a.x?.[1] === b.x?.[1] && a.y?.[0] === b.y?.[0] && a.y?.[1] === b.y?.[1];
}

/** A column's values for a filter pick-list. */
export interface DistinctValues {
  readonly values: readonly DistinctValue[];
  /** More values match than were sent. */
  readonly truncated: boolean;
}

interface Pending<T> {
  readonly resolve: (value: T) => void;
  readonly reject: (error: Error) => void;
}

function sameViewport(a: Viewport | null, b: Viewport): boolean {
  return a !== null && a.firstRow === b.firstRow && a.rowCount === b.rowCount && a.firstColumn === b.firstColumn && a.columnCount === b.columnCount;
}

/** One open view. */
export class ViewHandle {
  private configVersion = 0n;
  private lastLoadVersion = 0n;
  /** The viewport request awaiting its frame, and when the grid asked for it. */
  private inFlight: { readonly viewport: Viewport; readonly requestId: bigint; readonly askedAtMs: number } | null = null;
  /** A viewport asked for while another was in flight, and when. */
  private pending: { readonly viewport: Viewport; readonly askedAtMs: number } | null = null;
  private closed = false;
  /** The chart window awaiting its frame, and one asked for meanwhile, as with viewports. */
  private chartInFlight: { readonly window: ChartWindow; readonly requestId: bigint; readonly askedAtMs: number } | null = null;
  private chartPending: { readonly window: ChartWindow; readonly askedAtMs: number } | null = null;
  private readonly fetches = new Map<bigint, Pending<FrameData>>();
  private readonly lists = new Map<bigint, Pending<DistinctValues>>();

  constructor(
    public readonly id: number,
    private readonly connection: Connection,
    private readonly listener: ViewListener
  ) {}

  /** The most recent window asked for, whether answered yet or not. */
  public requested(): Viewport | null {
    return this.pending?.viewport ?? this.inFlight?.viewport ?? null;
  }

  /** Asks for a window of rows and data columns. */
  public setViewport(viewport: Viewport): void {
    if (this.closed || sameViewport(this.pending?.viewport ?? null, viewport)) {
      return;
    }
    // Back to the window in flight: whatever was waiting is no longer wanted.
    if (sameViewport(this.inFlight?.viewport ?? null, viewport)) {
      this.pending = null;
      return;
    }
    // The clock starts now, so time spent waiting behind the request in flight counts.
    const askedAtMs = performance.now();
    if (this.inFlight !== null) {
      this.pending = { viewport, askedAtMs };
      return;
    }
    this.sendViewport(viewport, askedAtMs);
  }

  /**
   * Asks for a chart's reduction. While one is in flight, a newer window replaces any waiting one, so zooming
   * and panning faster than frames arrive never queues them.
   */
  public setChartWindow(window: ChartWindow): void {
    if (this.closed || sameChartWindow(this.chartPending?.window ?? null, window)) {
      return;
    }
    // Back to the window in flight: whatever was waiting is no longer wanted.
    if (sameChartWindow(this.chartInFlight?.window ?? null, window)) {
      this.chartPending = null;
      return;
    }
    const askedAtMs = performance.now();
    if (this.chartInFlight !== null) {
      this.chartPending = { window, askedAtMs };
      return;
    }
    this.sendChartWindow(window, askedAtMs);
  }

  private sendChartWindow(window: ChartWindow, askedAtMs: number): void {
    const requestId = this.connection.send({
      case: "setChartWindow",
      value: {
        viewId: this.id,
        ...(window.x === undefined ? {} : { xFrom: window.x[0], xTo: window.x[1] }),
        ...(window.y === undefined ? {} : { yFrom: window.y[0], yTo: window.y[1] }),
        width: Math.max(1, Math.round(window.width)),
        height: Math.max(1, Math.round(window.height))
      }
    });
    this.chartInFlight = { window, requestId, askedAtMs };
  }

  /** @internal Called by the connection. */
  public receiveChartFrame(frame: ChartFrameData, requestId: bigint): void {
    let askedAtMs: number | undefined;
    if (this.chartInFlight?.requestId === requestId) {
      askedAtMs = this.chartInFlight.askedAtMs;
      this.chartInFlight = null;
      const next = this.chartPending;
      this.chartPending = null;
      if (next !== null) {
        this.sendChartWindow(next.window, next.askedAtMs);
      }
    }
    if (frame.configVersion < this.configVersion || frame.loadVersion < this.lastLoadVersion) {
      return;
    }
    this.lastLoadVersion = frame.loadVersion;
    this.listener.chart?.(askedAtMs === undefined ? frame : { ...frame, requestedAtMs: askedAtMs });
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

  /** Lists a column's values, with the view's other filters applied, for a filter pick-list. */
  public distinctValues(column: string, search: string, limit: number): Promise<DistinctValues> {
    if (this.closed) {
      return Promise.reject(new Error("the view is closed"));
    }
    return new Promise((resolve, reject) => {
      const requestId = this.connection.send({ case: "distinctValues", value: { viewId: this.id, column, search, limit } });
      this.lists.set(requestId, { resolve, reject });
    });
  }

  /** Fails every request still waiting for a reply. */
  private failPending(reason: string): void {
    for (const pending of [...this.fetches.values(), ...this.lists.values()]) {
      pending.reject(new Error(reason));
    }
    this.fetches.clear();
    this.lists.clear();
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
      this.failPending("the view is closed");
      this.connection.send({ case: "closeView", value: { viewId: this.id } });
      this.connection.forget(this.id);
    }
  }

  private sendViewport(viewport: Viewport, askedAtMs: number): void {
    const requestId = this.connection.send({
      case: "setViewport",
      value: { viewId: this.id, firstRow: BigInt(viewport.firstRow), rowCount: BigInt(viewport.rowCount), firstColumn: viewport.firstColumn, columnCount: viewport.columnCount }
    });
    this.inFlight = { viewport, requestId, askedAtMs };
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
      this.sendViewport(next.viewport, next.askedAtMs);
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
    const askedAtMs = this.inFlight?.requestId === requestId ? this.inFlight.askedAtMs : undefined;
    this.settle(requestId);
    if (frame.configVersion < this.configVersion || frame.loadVersion < this.lastLoadVersion) {
      return;
    }
    this.lastLoadVersion = frame.loadVersion;
    this.listener.frame(askedAtMs === undefined ? frame : { ...frame, requestedAtMs: askedAtMs });
  }

  /** @internal Called by the connection when the transport closes: fetches fail and the view reports why. */
  public receiveDisconnect(reason: string): void {
    this.failPending(reason);
    this.inFlight = null;
    this.pending = null;
    this.closed = true;
    this.listener.error?.(reason);
  }

  /** @internal Called by the connection. */
  public receiveDistinct(result: DistinctValuesResult, requestId: bigint): void {
    const pending = this.lists.get(requestId);
    if (pending === undefined) {
      return;
    }
    this.lists.delete(requestId);
    try {
      pending.resolve({ values: decodeDistinct(result.values), truncated: result.truncated });
    } catch (error: unknown) {
      pending.reject(error instanceof Error ? error : new Error(String(error)));
    }
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
    const pending = this.fetches.get(requestId) ?? this.lists.get(requestId);
    if (pending !== undefined) {
      this.fetches.delete(requestId);
      this.lists.delete(requestId);
      pending.reject(new Error(message));
      return;
    }
    this.settle(requestId);
    if (this.chartInFlight?.requestId === requestId) {
      this.chartInFlight = null;
      const next = this.chartPending;
      this.chartPending = null;
      if (next !== null) {
        this.sendChartWindow(next.window, next.askedAtMs);
      }
    }
    this.listener.error?.(message);
  }
}

/** A protocol connection over one transport. */
export class Connection {
  private nextRequest = 1n;
  private nextView = 1;
  private readonly views = new Map<number, ViewHandle>();
  private welcome: Welcome | null = null;
  private readonly welcomed: Promise<Welcome>;
  private settleWelcome: { readonly resolve: (welcome: Welcome) => void; readonly reject: (error: Error) => void } | null = null;
  private readonly errors: ((message: string) => void)[] = [];

  /** `location` says where the engine behind `transport` runs. */
  constructor(
    private readonly transport: Transport,
    public readonly location: EngineLocation = "server"
  ) {
    this.welcomed = new Promise((resolve, reject) => {
      this.settleWelcome = { resolve, reject };
    });
    // A connection that closes before its welcome rejects; nobody may be waiting.
    this.welcomed.catch(() => undefined);
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

  /** Waits for the engine's welcome and returns what it says about `name`, or undefined if it has no such dataset. */
  public async dataset(name: string): Promise<DatasetInfo | undefined> {
    const welcome = await this.welcomed;
    return welcome.datasets.find((d) => d.name === name);
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
    this.settleWelcome?.reject(new Error(reason));
    this.settleWelcome = null;
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

  /** Decodes a frame for its view, reporting a frame that fails its checks as that view's error. */
  private receiveFrame(encoded: Frame, requestId: bigint): void {
    const view = this.views.get(encoded.viewId);
    if (view === undefined) {
      return;
    }
    let frame: FrameData;
    try {
      frame = decodeFrame(encoded);
    } catch (error: unknown) {
      view.receiveError(error instanceof FrameError ? error.message : String(error), requestId);
      return;
    }
    view.receiveFrame(frame, requestId);
  }

  /** Decodes a chart frame for its view, reporting one that fails its checks as that view's error. */
  private receiveChartFrame(encoded: ChartFrame, requestId: bigint): void {
    const view = this.views.get(encoded.viewId);
    if (view === undefined) {
      return;
    }
    let frame: ChartFrameData;
    try {
      frame = decodeChartFrame(encoded);
    } catch (error: unknown) {
      view.receiveError(error instanceof FrameError ? error.message : String(error), requestId);
      return;
    }
    view.receiveChartFrame(frame, requestId);
  }

  /** Handles one encoded server message. */
  public receive(bytes: Uint8Array): void {
    const message = fromBinary(ServerMessageSchema, bytes);
    const requestId = message.requestId;
    const body = message.body;
    switch (body.case) {
      case "welcome":
        this.welcome = body.value;
        this.settleWelcome?.resolve(body.value);
        this.settleWelcome = null;
        return;
      case "viewOpened":
        this.views.get(body.value.viewId)?.receiveOpened(body.value);
        return;
      case "frame":
        this.receiveFrame(body.value, requestId);
        return;
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
        this.views.get(body.value.viewId)?.receiveDistinct(body.value, requestId);
        return;
      case "chartFrame":
        this.receiveChartFrame(body.value, requestId);
        return;
      case undefined:
        return;
    }
  }
}
