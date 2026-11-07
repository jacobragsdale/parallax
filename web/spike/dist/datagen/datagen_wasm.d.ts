/* tslint:disable */
/* eslint-disable */

/**
 * Tables and a protocol session, for one page.
 */
export class Worker {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Writes the layout's view as CSV and returns its bytes, as
     * [`Worker::export_xlsx`] does for Excel.
     *
     * # Errors
     *
     * Returns the reason the export cannot be made, as the server would.
     */
    exportCsv(layout_json: string, load_version?: number | null): Uint8Array;
    /**
     * Writes the layout's view as an Excel workbook, held in memory, and returns
     * its bytes. The layout names its dataset and parameters; `load_version` is
     * the load the screen shows, and an export of any other load is refused.
     *
     * # Errors
     *
     * Returns the reason the export cannot be made, as the server would.
     */
    exportXlsx(layout_json: string, load_version?: number | null): Uint8Array;
    /**
     * Handles one encoded client message and returns the encoded replies.
     */
    handle(message: Uint8Array | number[]): Array<any>;
    /**
     * Loads a copy of a server table, as `GET /copy/{name}` sends it, for one set
     * of parameters given as a JSON object of strings. Views of the copy show what
     * views of the server's table show, status included. A second copy for the
     * same name and parameters replaces the first for views opened afterwards.
     *
     * # Errors
     *
     * Returns an error if the parameters are not an object of strings, or the bytes
     * are not a copy.
     */
    loadCopy(name: string, parameters_json: string, copy: Uint8Array | number[]): bigint;
    /**
     * Spike 0.3: loads a dataset as [`Worker::load_table`] does, but decodes the
     * stream in place, so that its arrays point into the bytes rather than
     * copying them, and frees the bytes as soon as the table holds the rows.
     *
     * # Errors
     *
     * Returns an error if the stream is invalid or the load is refused.
     */
    loadTableInPlace(name: string, arrow_ipc: Uint8Array | number[], primary_key: string | null | undefined, now_micros: number, source_as_of_micros: number): bigint;
    /**
     * Loads or reloads dataset `name` from an Arrow IPC stream. A reload of an
     * existing dataset applies as a diff to its open views on the next
     * [`Worker::refresh`].
     *
     * # Errors
     *
     * Returns an error if the stream is invalid or the load is refused.
     */
    loadTable(name: string, arrow_ipc: Uint8Array | number[], primary_key: string | null | undefined, now_micros: number, source_as_of_micros: number): bigint;
    /**
     * Creates a worker with no tables.
     */
    constructor();
    /**
     * Brings open views up to date with reloaded tables and returns the pushed
     * messages.
     */
    refresh(): Array<any>;
}

/**
 * Spike 0.3 only: generated positions as an Arrow IPC stream, the bytes an
 * app's API would send, so that the measurement page needs no server.
 *
 * # Errors
 *
 * Returns an error if the options are invalid or the stream cannot be written.
 */
export function generatePositions(rows: number, seed: number): Uint8Array;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_worker_free: (a: number, b: number) => void;
    readonly generatePositions: (a: number, b: number) => [number, number, number, number];
    readonly worker_exportCsv: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly worker_exportXlsx: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly worker_handle: (a: number, b: number, c: number) => any;
    readonly worker_loadCopy: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [bigint, number, number];
    readonly worker_loadTable: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [bigint, number, number];
    readonly worker_loadTableInPlace: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [bigint, number, number];
    readonly worker_new: () => number;
    readonly worker_refresh: (a: number) => any;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
