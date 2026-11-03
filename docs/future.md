# Deferred work

Each item here was considered during design on 2026-11-02 and deliberately left
out of the current milestones. Each one lists the reason it waits and any
research already done.

## Data sources

- **Data lake connector.** Which lake is in use (ADLS Gen2, S3, Databricks or
  Delta, Fabric OneLake) is unknown. The `object_store` crate covers the object
  stores once the lake is known.
- **Snowflake.** The planned move to Snowflake hasn't happened yet. Its ADBC
  driver moved from Apache Arrow to the ADBC Driver Foundry in ADBC 24
  (2026-07-28). `adbc_core` 0.25 is the Rust entry point.
- **Live sources and RabbitMQ.** No streaming source exists, although RabbitMQ
  has been used before. When one arrives, it gets an adapter on the ingest
  interface, along with the live-update budget of 10,000 row updates per second
  with a p99 of ≤ 250 ms from update to screen.
- **Converting JSON inside SQL Server.** `OPENJSON ... WITH` could move JSON
  conversion onto the database server. The first milestone converts JSON in
  Rust, with the same code that handles MongoDB documents.
- **Arrays in documents.** All arrays are rejected for now, because the engine
  has no list columns. Arrays of scalars need list columns in the engine and the
  grid; arrays of objects also need a choice between exploding them into child
  rows and using Arrow struct lists.

## Engine

- **Computed columns.** A small expression language compiled into column
  operations. Planned for v2.
- **Running views as SQL in another engine.** View configurations would be
  translated to SQL for DuckDB or ClickHouse, as Perspective's virtual servers
  do.
- **Refreshing copies in the browser.** A copy is fetched once per page, so a
  reload on the server reaches a browser-held view only when the page reloads.
  Pushing reloads to copies, as a diff, waits until pages stay open long enough
  for it to matter. Copies are also never evicted from the worker during a page's
  life.
- **Browser threads and 64-bit WebAssembly memory.** Rust's
  `wasm64-unknown-unknown` target is Tier 3. Threads need nightly Rust,
  `-Z build-std` and cross-origin isolation headers. Revisit if a
  browser-resident dataset outgrows 4 GB or one thread.

## Interface

- **Cell editing and writeback.**
- **Native Excel pivot tables.** `rust_xlsxwriter` 0.102 doesn't write them.
  Supporting them means writing the pivot cache and pivot table parts of the
  file format directly.
- **Screen-reader accessibility.** Not required for the first milestone. The
  grid experiment's choice between canvas and DOM affects the cost.

## Clients

- **Python package with Streamlit and Dash components.** Python is dropped for
  now, but these findings from 2026-11-02 still apply:
  - PyO3 0.30.1, maturin 1.16.0 and `pyo3-arrow` 0.20.0 can expose the engine,
    server and export without copying data from polars or pyarrow.
  - Streamlit 1.66 Components v2 render inside the page without an iframe and
    pass Streamlit's theme through CSS custom properties. A custom-element
    build of the Angular components could mount there directly.
  - Dash 4.5 components must be React, so Dash needs a thin React wrapper
    around the same custom element.
  - The components must talk to the engine over their own WebSocket. Routing
    scroll and pivot traffic through Python would rerun Streamlit scripts and
    fire Dash callbacks.
  - Dash under gunicorn runs several workers, so it needs a client mode that
    connects to a separate server. Single-process Streamlit can run the server
    inside Python.
  - A server started from Python needs at least localhost binding and
    unguessable per-view tokens, because the library has no auth.
- **Jupyter widget.** Not wanted.
