# How parallax is designed

parallax is a data grid, pivot table and charting stack for portfolio managers
and quant researchers. It is a Rust engine with an Angular front end. Three
facts about its users drive the design. The datasets are large. The browsers
have no GPU. A wrong number on screen costs more than a slow one.
[Perspective](https://github.com/perspective-dev/perspective) is the model for
the architecture. parallax is a new implementation, not a fork, and its tests
use Perspective as a second reference.

## Why existing grids fail at this size

The component suites that parallax replaces hold each row as a JavaScript
object and draw one DOM or SVG node per value. DevExpress DevExtreme is the
one in the first deployment. One million rows of 20 floating-point columns and
two text columns were parsed from JSON in Node 26's V8 on 2026-11-02. They
took 509 MB of JavaScript heap, about 530 bytes a row. Stored as typed
columns, the same data took 155 MB, 163 bytes a row.

V8's pointer compression caps a tab's JavaScript heap near 4 GB. Grid suites
also keep extra copies of the rows for sorting, filtering and wrapping, so a
few million rows exhaust the tab. The JSON text adds to the peak: about 510 MB
for that million rows, held alongside the parsed objects while it parses.

parallax avoids each of those costs. Data travels and lives as Arrow columns.
Computation happens where the data is held. The screen receives only what it
draws.

## The environment: Windows 11 on Citrix, without a GPU

Users run a Chromium-based browser on 64-bit Windows 11 Citrix VDIs, with
software rendering only. Chromium
[deprecated](https://groups.google.com/a/chromium.org/g/blink-dev/c/yhFguWS_3pM/m/oLrB5up_BwAJ)
the automatic SwiftShader fallback for WebGL in Chrome 130 and removed it by
Chrome 139. On a machine without a GPU, creating a WebGL context therefore
fails outright instead of running slowly. WebGPU is unavailable for the same
reason. That rules out every GPU charting library, Perspective's WebGL charts
included. Canvas 2D, which Skia rasterizes on the CPU, is the drawing surface
that works.

Citrix adds two more pressures. A VDI's CPU and memory are shared and limited,
so the browser should hold as little data as possible. Citrix's display
protocol sends only the screen regions that changed. A redraw confined to the
changed cells therefore costs less than a full repaint, both on the VDI and on
the network.

## One engine, two places to run

The engine is a Rust library compiled two ways: natively for the server, and
to `wasm32-unknown-unknown` for the browser, where it runs in a Web Worker off
the main thread. Both builds sit behind the same client protocol, so a dataset
can choose where it lives without the UI noticing. Small datasets are copied to
the browser, where pivots respond without a network round trip. Large datasets
stay on the server, and the browser receives only the visible window.

The browser build is single-threaded and stays under the 4 GB limit of 32-bit
WebAssembly. Going further would cost a lot:

- Rust's `wasm64-unknown-unknown` target is Tier 3.
- Multithreaded WebAssembly from Rust needs a nightly compiler and a rebuilt
  standard library.
- Threads also need cross-origin isolation headers on the host application.
- The `wasm-bindgen-rayon` helper was last released in December 2024.

That cost isn't worth paying when large data belongs on the server anyway. The
server build parallelizes with rayon.

## Inside the engine

A table stores each column as a typed vector with a validity bitmap, one entry
per slot. A slot is stable for the life of a row:

- an update overwrites the slot in place
- a delete frees it for reuse
- a hash index on the primary key finds the slot for a key

Text columns are dictionary-encoded. Each dictionary keeps a rank for every
string in the engine's text order, so sorting and grouping compare integers
instead of strings. Arrow is the format at the edges: loads arrive as Arrow
batches, and every window leaves as one.

A load into a keyed table is a diff against the current contents. New keys are
inserted, changed rows are updated in place, and missing keys are deleted. The
load is validated in full before anything changes. The result is a change set:
the old images of removed and overwritten rows, and the slots of new and updated
ones.

A view is a pipeline over a table:

1. filter
2. computed columns
3. group or pivot
4. aggregate
5. sort
6. the window the screen asks for

A grouped view applies a change set as a diff, as Perspective does. It removes
the old images' contributions and adds the new rows'. That gives the same
state as building the view from scratch, which the tests check after random
streams of loads and upserts. Live streams use this, but its main use is
refresh. About 90% of pages show snapshot data, and applying a reload as a diff
keeps an open pivot's expanded rows, sort and scroll position intact.

A flat view re-filters and re-sorts the table on every change, using every core
on the server. Incremental maintenance of flat views waits for live sources,
which are deferred.

A grouped view keeps a tree. The root is the grand total, and each level below
groups by one more column. Every node is fed directly by base rows, so no total
is computed from other totals. Column pivots keep one cell per node and
column-pivot key, next to the node's own row total.

The aggregates are sum, count, mean, weighted mean, minimum, maximum, first,
last, distinct count, variance, standard deviation, median and percentiles.

- **Sum, count, mean, variance and weighted mean** are kept at every node as
  exact sums that can be reversed. A change costs constant time per node.
- **Minimum, maximum, first, last, distinct count, median and percentiles** keep
  value counts at the leaf nodes only. Nodes above merge their leaves when a
  value is needed, which keeps memory proportional to the rows rather than to
  rows times levels.

Pivots support:

- several row and column levels
- subtotals and grand totals
- expand and collapse state that survives refreshes and re-sorts
- sorting by any aggregate within a level
- filters on raw values and on aggregates
- top-N per level, with an "Other" row
- percentage of parent, row, column or grand total
- drill-through to the underlying rows
- several measures at once

## What "never wrong" means

parallax treats a plausible but wrong number as worse than an error. Every rule
in this section follows from that.

### Numbers

Money columns use exact decimals (Arrow Decimal128) alongside f64, i64 and
time-zone-aware timestamps. Floating-point sums are exact until one final
rounding: each double is added to a fixed-point integer wide enough for every
finite double (a Kulisch accumulator), and the total is rounded once. The `xsum`
crate, the first choice, returned a neighbouring double for some single values,
so the engine carries its own accumulator and the tests compare it with a port of
CPython's `math.fsum`. A total comes out the same, to the last bit, however it is
computed:

- incrementally
- from scratch
- in the browser
- on the server
- in DuckDB

Removing a value adds its negative, which is equally exact. Integer and
decimal sums accumulate in 256-bit integers. A decimal sum that cannot fit 38
digits shows an overflow marker instead of a wrapped value. A mean is the exact
sum divided by the count, rounded once.

The sample variance of doubles comes from the exact sums of values and of
squares, each square added without error. Big-integer arithmetic then divides
once. A group of identical values therefore has a variance of exactly zero,
not a rounding residue. Percentiles of decimals are reported as doubles at full
interpolated precision. DuckDB instead rounds them to the column's scale.

Every subtotal and grand total is computed from base rows, never from other
subtotals, so no average is an average of averages. Percent-of-total with a
zero denominator shows a dash. Distinct counts are exact. Percentiles
interpolate linearly, matching Excel's `PERCENTILE.INC` and DuckDB's
`quantile_cont`.

A column can declare its unit, either fixed (`USD`) or taken from another
column (`currency`). When a group mixes units, the cell reads "mixed" and lists
the units on hover. The exception is when the layout converts through a
declared FX-rate column. The same guard covers percent against basis points.

### Nulls, NaN and text

Nulls follow SQL rules:

- Aggregates skip nulls.
- `count(*)` and `count(column)` stay distinct.
- A null group key appears as "(blank)" and sorts last.

NaN becomes null at load time and is counted in the view's data-quality badge,
so the substitution is visible. Positive and negative infinity are kept.

Text sorts case-insensitively, with a case-sensitive tie-break and nulls last.
There is no locale-dependent collation, so the order is the same on every
machine and matches the test oracle.

### Display

The screen and the Excel export show the same digits because both use one
formatter, in the engine. Frames carry its text next to each value, so the
browser draws what the engine wrote and never formats a number itself. It
follows Excel's display rules: a double is rounded to 15 significant digits, then
to the shown decimals, both half away from zero. So 2.675 at two decimals shows
as 2.68 in both.

Exact decimals and integers skip the 15-digit step: they never were doubles. A
decimal of 15 significant digits or fewer converts to a double and back
unchanged, so Excel shows the same digits. A longer one shows in full on screen;
how the export writes it is decided with the export.

- A value that rounds to zero never shows a minus sign.
- A number too wide for its column shows `###`, as Excel does, never with
  digits clipped off.
- Text truncates with an ellipsis and shows in full on hover.
- K/M/B abbreviations are opt-in per column and always show the suffix.
- Percent and K/M/B scaling move the decimal point exactly, before the one
  rounding to the shown decimals.
- Booleans show as `TRUE` and `FALSE`, as in Excel.
- Copy and export carry full-precision values. Display formats travel as Excel
  number formats.

A layout sets formats per source column: decimals, thousands separators,
percent, K/M/B, negatives in parentheses, the display zone, and sub-second
digits. A measure takes its column's format unless it sets its own; counts and
variances use their type's default, and shares of a total show as percentages.

Rounding happens only in this formatter. The tests compare it with exact
big-integer arithmetic on random doubles, many of them next to rounding
midpoints, and compare its time zones with DuckDB's.

### Time

Timestamps are stored as UTC instants. They are displayed in a zone that each
column declares, defaulting to `America/New_York`, and headers name the zone.
Digits below the shown precision are cut rather than rounded, so a time never
shows later than it happened and 23:59:59.9 never becomes the next day.

SQL Server `datetime` and `datetime2` values carry no zone, so each such column
declares its source zone, also defaulting to New York. Daylight-saving changes
produce local times that are ambiguous or don't exist:

- In autumn, local times from 01:00 to 01:59 occur twice. Such a time is a load
  error unless the column declares whether to take the earlier or the later
  instant.
- In spring, an hour of local time is skipped. A time in that gap is always an
  error.

Business dates such as trade date stay dates and never pass through a zone.
Chart buckets are cut in the display zone, so a day is a New York day. The
engine uses `jiff` with a bundled, version-pinned time-zone database, so the
server and browser builds agree. Diagnostics show the database version.

### Loads

A load that would change the meaning of the data fails loudly instead:

- Duplicate primary keys are an error, unless the dataset opts into
  last-one-wins.
- A column that changes type or disappears is an error naming the column.
- A document field whose value has the wrong type is an error naming the row
  key, path and value.
- Oversized text is an error, never a truncation.

A load that fails partway never replaces the last good table. The view keeps
the old data and shows a "refresh failed" badge with the time. Each table
records its row count, a hash of the source query, its load times and a
checksum, all visible in the view's info panel.

Every frame of rows sent to the screen carries two versions: the view
configuration that produced it, and the table load. The UI discards frames
that don't match the current versions. A scroll racing a filter change
therefore can never paint old rows under new headers, and a view never mixes
rows from two loads.

## How the guarantees are tested

The tests compare parallax with independent implementations rather than with
expected values written by hand. They run on data from a reproducible,
seed-driven generator. It produces realistic finance data:

- positions, trades, and daily prices with fat-tailed returns
- Zipf-distributed ticker frequency
- several currencies and mixed decimal scales

It also produces hostile values: nulls, NaN, infinities, negative zero,
notionals of 10^15, weights of 10^-12, duplicate keys, Unicode tickers, and
timestamps at daylight-saving transitions.

Six layers of tests run on that data:

1. Random tables and random view configurations run through parallax and
   through DuckDB, and the results must match. Perspective is a second oracle.
2. Random streams of updates must leave every view identical to a
   recomputation from scratch.
3. The native and WebAssembly builds must return identical results.
4. The cells the grid draws must match the engine's output. This is checked
   from the renderer's cell model, and with Playwright screenshots in Chromium
   with the GPU disabled.
5. Exported Excel files are read back with `calamine` and compared with the
   view.
6. The protocol is fuzzed.

Technical-analysis indicators are checked against TA-Lib. Where indicator
definitions vary, such as how an EMA starts, parallax follows TA-Lib's
convention of seeding with an SMA. Excel can't run on the Linux build host, so
at each milestone someone opens a verification workbook in real Excel on a VDI.

## Drawing without a GPU

The engine reduces data to screen resolution before sending it, so the client
never draws more than the screen can show.

Line charts use M4 aggregation. It keeps the first, last, minimum and maximum
value in each pixel column, and the resulting line matches, pixel for pixel,
the line through every point at that width. Spikes never vanish. A chart 2,000
pixels wide therefore needs at most 8,000 points, whether the series has 2,000
points or 20 million.

Scatter plots and heatmaps become binned densities, and bar charts arrive
aggregated. Indicators are computed on full-resolution data before the
reduction. A 200-day moving average computed from downsampled points would be
wrong.

The client draws with Canvas 2D:

- only on animation frames
- only in regions that changed
- with updates batched to a configurable visual rate, 4 Hz by default for
  flashing cells

Renderers, hit-testing and interaction are TypeScript inside the Angular
library. Drawing from Rust would cross the WebAssembly boundary on every text
call and make theming harder.

The grid draws on a canvas too. A virtualized DOM grid, Perspective's choice, was
measured against it on the same views in Chromium with the GPU disabled and the
CPU throttled four times. The DOM grid spent 1.1 to 2 times as much main-thread
time per frame, which is the headroom a slow desktop has before frames drop.
[The grid experiment](grid-experiment.md) has the method and the numbers. The DOM
grid brings native text selection and accessibility, which the canvas grid
provides separately; screen-reader support isn't a first-milestone requirement.

## The wire protocol

Each browser tab opens one binary WebSocket and carries all of its views over
it, tagged by view ID.

- Control messages are protobuf: create a view, change its configuration, set
  the visible window, subscribe. The `.proto` files are the single source of
  truth and generate both the Rust and the TypeScript code.
- Data travels as Arrow IPC. Frames for the visible window are small and go
  uncompressed. Bulk loads into the browser are zstd-compressed and decoded in
  the worker.
- Every data frame carries the view ID, configuration version, window bounds,
  table load version and source timestamp.
- A client can fetch a window once without moving the view's viewport, which
  is how a copy larger than the screen gets its rows. One fetch is limited to
  1,000,000 cells.
- Each data column in a frame travels with its display text, its cell flags
  (overflow, mixed units, undefined) and, for timestamps, full-precision text
  for copying. Every Arrow field names its role and column index in its
  metadata, so the client pairs them by index, never by name. Column-pivot names
  can display alike: 1.001 and 1.004 both read 1.00 at two decimals.

The browser worker that hosts the WebAssembly engine speaks the same messages
over `postMessage`, through the same session code as the server. The server
and the worker therefore can't drift apart: a fixed conversation must produce
byte-identical replies from the native and WebAssembly builds.

gRPC-web was ruled out because browsers can't use it for bidirectional
streaming. WebTransport was ruled out because it runs over UDP, which corporate
proxies often block.

## The server and its data

`parallax-server` is a crate that a host application embeds. It exposes an
axum router and an engine handle. A small reference binary wraps it for
deployment as a Linux container on Kubernetes (AKS), close to the Citrix farm
on the network.

The library contains no authentication, entitlement or audit code. Its one
extension point is a per-connection value: the host's middleware sets it,
dataset code receives it, and the library passes it through without
interpreting it.

A host registers datasets through a Rust API, and the reference binary reads
the same definitions from TOML. A definition names the source, query,
parameters, primary key, schema and refresh policy.

Tables are cached in memory and shared across users, keyed by dataset and
parameters, so fifty people opening the same page cost one load. A table
refreshes in one of three cases:

- it is older than a time limit when a view opens
- its schedule says so
- a publisher calls an invalidation endpoint

Each refresh applies as a diff. A reload fetches without holding up readers:
views keep showing the current rows until the new ones apply, and tables due at
the same time reload in parallel, so one slow source does not delay the others.
A memory budget evicts the least recently used tables, and an Arrow copy on
local SSD makes reloading fast. Existing Python
and C# services keep their business logic. parallax pulls from their existing
GET endpoints through the HTTP connector, and live sources push Arrow batches
through an ingest API.

The connectors live in the `parallax-sources` crate, behind one interface: a
source reads every row for a set of parameters in one call. The reference
server's TOML names them by `kind`; `examples/sources.toml` shows each. The first
connectors cover where the data already lives:

- **SQL Server** goes through `arrow-odbc` and Microsoft ODBC Driver 18, which
  fetch column batches instead of rows. SQL Server 2025's
  [`json` type](https://learn.microsoft.com/en-us/sql/t-sql/data-types/json-data-type)
  and JSON stored in `nvarchar(max)` both arrive as text. ODBC isn't among the
  clients that support the native type.
- **Oracle Database** goes through the same `arrow-odbc` path, with Oracle's
  Instant Client ODBC driver. Three Oracle behaviours meet the load rules in
  [What "never wrong" means](#what-never-wrong-means):
  - `NUMBER` without a declared precision is a floating decimal of up to 38
    digits, but Oracle's ODBC driver reports it as a double and converts it
    through one even when asked for text. Such a column must be selected as
    `TO_CHAR(col, 'TM9', 'NLS_NUMERIC_CHARACTERS=''.,''')` and declare its Arrow
    type, so the text made by the database is parsed exactly, never through f64.
  - `DATE` carries a time of day but no zone, so it follows the rule for
    zoneless timestamps. `TIMESTAMP WITH TIME ZONE` is an instant.
  - Oracle stores an empty string as NULL, so text columns from Oracle can't
    distinguish the two.
- Database connections and queries are configuration. A connection string may
  name environment variables as `${NAME}`, so passwords stay out of files, and
  dataset parameters bind to the query's `?` markers in a declared order. A
  column fetched as text to stay exact, a zoneless time's zone and choice in the
  repeated hour, a time kept as wall-clock, and a JSON column's fields are all
  declared per dataset. Digits below the microsecond refuse the load rather than
  being cut. Every column a setting names must be in the query's result, so a
  misspelt name fails instead of falling back to defaults. On Oracle, a column
  the driver reports as a double refuses the load unless it is listed as a real
  `BINARY_DOUBLE` or declared as a float, and the refusal names the `TO_CHAR`
  form to select instead. An environment value is quoted for its setting: braced in an ODBC
  string, percent-encoded in a MongoDB URI; `env:NAME` takes a whole setting from
  the environment. The ODBC and MongoDB connectors are features of `parallax-sources`
  and the server, because they need unixODBC and the database drivers
  installed. CI runs them against SQL Server, Oracle Database and MongoDB in
  service containers.
- **JSON columns and MongoDB documents** go through one shared converter that
  maps declared paths to declared Arrow types.
  - Numbers are parsed from text straight into the declared type, and decimals
    are parsed exactly, never through f64.
  - A missing path is null.
  - Nested fields are reached by dotted paths. Arrays are refused for now,
    because the engine has no list columns yet; see
    [Deferred work](future.md).
  - Wall-clock text without an offset follows the zoneless-timestamp rule in
    the column's zone.
  - MongoDB uses the official `mongodb` driver. `Decimal128` values keep every
    digit, dates become instants, and object ids become their hex text. The
    filter is extended JSON with `{name}` placeholders, filled as JSON strings
    so that a parameter cannot change the filter's shape.
- **Parquet** goes through the `parquet` crate, behind a local SSD cache keyed
  by path, size and modification time, so the slow NFS share is read only once.
  Older copies of a file are removed when a newer one is cached. Dataset
  parameters may fill placeholders in the path, but not with path separators or
  a leading dot, so a parameter cannot reach outside the configured directory.
- **HTTP** pulls from the existing C# and Python REST GET endpoints, on the
  same refresh rules as any other source.
  - Dataset parameters map onto the GET's query string, so a table is still
    cached by dataset and parameters.
  - The host supplies a service credential, as it supplies the per-connection
    value: a header read from an environment variable at each fetch, or a
    function that returns headers. The library still contains no auth code.
  - Certificates are checked against the operating system's trust store, so
    endpoints signed by the company's own authority work. A response larger
    than the dataset's limit, 2 GiB by default, refuses the load.
  - The connector asks for an Arrow IPC stream with
    `Accept: application/vnd.apache.arrow.stream`, the
    [IANA-registered type](https://www.iana.org/assignments/media-types/application/vnd.apache.arrow.stream),
    and falls back to JSON, which goes through the shared JSON converter. A
    service can add Arrow beside JSON on the same route, with pyarrow in Python
    or the Apache.Arrow NuGet package in C#, and its other callers keep getting
    JSON.

### Choosing a source

Pulling suits snapshot data better than pushing, because a service can't know
in advance which parameters a PM will open, such as an as-of date and a fund.
Push ingest stays for live streams and for "data changed" signals.

Which connector a dataset uses depends on what its existing GET does:

- A GET that wraps one query or file: connect directly with the SQL Server,
  Oracle, Parquet or MongoDB connector, and move the SQL into the dataset
  definition.
- A GET that carries business logic, such as calculations, joins across sources,
  adjustments or Python models: use the HTTP connector. Rewriting that logic as
  SQL inside parallax would let the grid and the app's other pages show
  different numbers for the same thing.
- A GET that filters rows by the calling user: tables are shared across users,
  so it needs an unfiltered variant called with the service credential, with
  the host applying entitlements. The alternative, per-user parameters, gives up
  the sharing. Whether the existing GETs filter by user is still an open
  question.

JSON costs far more than Arrow at this size. 100,000 rows of 20 floats and 2
strings were serialized in Python on 2026-11-02:

| Serializer | Time | Size |
|---|---|---|
| Standard library `json.dumps` | 873 ms | 56 MB |
| orjson | 96 ms | 52 MB |
| pyarrow IPC stream | 33 ms | 18 MB |

JSON is workable up to about a million rows. Past that the endpoint needs Arrow:
twenty million trades would be about 11 GB of JSON per load.

HTTP responses follow the load rules in [Loads](#loads), plus these:

- **Pagination:** pages fetched one after another can come from different
  moments. A load must be a single response, or pages pinned to one snapshot by
  an as-of parameter.
- **Decimals:** they arrive as JSON numbers or strings, and both are parsed
  exactly. Python's standard `json` module can't serialize `Decimal`, so
  services often convert money to float first. Those values have already passed
  through a double, which keeps 15 to 17 significant digits. Pydantic v2 writes
  decimals as strings.
- **Zoneless datetimes:** these follow the zoneless-timestamp rule. Pydantic
  and orjson both emit `2026-11-01T01:30:00`, which falls in New York's repeated
  hour, so it fails unless the column declares the earlier or later instant.
- **Bare `NaN`:** Python's `json.dumps` writes a bare `NaN` token by default,
  which isn't valid JSON. The converter accepts it as NaN, which becomes null and
  is counted, as for other sources.
- **Missing or changed fields:** these are load errors naming the column.

## The Angular library

The library targets Angular 22 and later, zoneless, with signals and
standalone components.

- `<px-grid>`, `<px-pivot>` and `<px-chart>` each take a single `layout` and
  emit `layoutChange`, so `[(layout)]` keeps the host's copy current.
- `provideParallax({ url })` configures the connection.
- A `ParallaxClient` service offers the same operations to code.
- The npm package ships the WebAssembly files.

A layout is a JSON document that describes a whole view: dataset and
parameters, grouping, pivots, filters, sorts, formats, column widths, chart
configuration and drawings. Saved views, shared URLs, Excel exports and tests
all use it. Its JSON Schema is generated from the Rust types, versioned with a
tested migration for each change, and committed as `schema/layout.schema.json`.
The TypeScript library checks layouts with a Zod schema that mirrors the Rust
types. A test reads layouts the Rust crate wrote and requires each to come back
unchanged, with the same defaults, so the two can't drift.

`<px-grid>` keeps everything the user changes in the layout. Clicking a header
sorts by that column, then reverses it, then clears it; Shift adds a sort. In a
pivot, a header sorts groups by that measure. Dragging a header edge resizes
the column, and double-clicking it fits the loaded values. Clicking a group
expands or collapses it. The grid sends the engine only what a change needs:

- New widths send nothing.
- Expanding or collapsing sends that group.
- New formats are applied without recomputing the view.
- Anything else recomputes it.

Cells are selected by click, Shift-click, drag or the keyboard: arrows, Page
Up and Page Down, Home and End, with Ctrl to jump and Shift to extend; Ctrl+A
selects everything. Ctrl+C copies the selection as tab-separated text with
every digit, which pastes into Excel as cells. A selection larger than the rows
on screen is fetched first, up to 1,000,000 cells, without moving the view.
Hovering over a cell whose text was cut, a `###`, or a flagged cell shows the
full text or what the flag means.

`<px-pivot>` puts the pivot's settings beside a grid: row and column groups in
order, values with their aggregate and how they are shown, filters, the levels
open by default, and top-N. Removing or moving a value renumbers everything that
referred to it by position: the group sort, aggregate filters, top-N and column
widths. A filter picks its values from a list of the column's values, which
the engine computes under the view's other filters, as a spreadsheet's filter
list does; text columns can search it. "(blank)" can be picked too: an "is one
of" list may name null, and then matches blanks. A filter stays out of the
layout until it names a value, because an empty "is one of" would hide every
row. Double-clicking a value opens the rows behind its cell in a drawer: a flat
view with the pivot's filters plus one filter per group level and per
column-pivot level. The drawer closes when the pivot's filters or groups
change, since its rows would no longer add up to the cells above.

A layout also travels as a link: `encodeLayout` deflates it into a URL
parameter, and `decodeLayout` checks it against the schema, so an edited link
fails loudly instead of opening a different view.

Under every grid a status bar names the dataset and the rows shown, and says
when the source was read and how long ago. It also flags NaN values the load
turned blank, and a failed refresh with its time. An info button lists the load
details.

Themes are design tokens defined once in the library, for a light and a dark
theme, and emitted as CSS custom properties in a cascade layer named
`parallax`. Any rule a host writes for `px-grid` therefore overrides them,
whatever its specificity. Canvas renderers read the computed values and redraw
when the theme changes. Compact and comfortable densities set the row height.

## Excel export

The server writes exports with `rust_xlsxwriter` and streams them as downloads,
because VDI memory is scarce: the page posts the layout in a form to
`POST /export`, and the browser saves the answer to disk instead of holding it.
An export runs the layout through the same engine and formatter as the screen,
so it holds the same numbers.

- Cells hold real numbers, with Excel number formats that show the digits the
  screen shows. A negative value that rounds to zero gets a format without a
  minus sign, because the screen never shows one there.
- Dates and times are Excel dates, shown in the display zone. Times shown to
  the microsecond, which Excel cannot hold, are written as their text.
- The sheet has a frozen header row, column widths from the layout, and, for
  flat views, an autofilter.
- A pivot exports every group as an outline, with the outline buttons beside
  the group rows, which come first as on screen. Groups start expanded:
  `rust_xlsxwriter` 0.102 marks a collapsed group on the row after it, which is
  right only for buttons below the groups. It also writes outline levels only for
  sheets it holds in memory, so pivots of up to 2,000,000 cells are outlined and
  larger ones stream without an outline, indented by level. Native Excel pivot
  tables are not written.
- A Notes sheet records the dataset, when the source was read, the layout, and
  every place Excel cannot match the screen: values with more than 15
  significant digits, which Excel rounds; infinities, which it lacks; and the
  like.
- An export larger than Excel's limit of 1,048,576 rows, or text longer than a
  cell's 32,767 characters, is refused with the reason, and CSV is offered
  instead. CSV holds every digit as text, and a pivot's CSV has one column per
  group level. It starts with a UTF-8 byte-order mark, without which Excel
  reads non-ASCII text in the system code page, and text that a spreadsheet
  would run as a formula gets a leading apostrophe.

The export reads the table one chunk at a time, each under its own read lock,
so a long export never holds up a reload; if the table reloads part way, the
export stops rather than mix two loads. A refusal reaches the page even when the
server is on another origin: the form carries a token, and the answer posts the
reason to the page, which shows it in the status bar.

The read-back tests open each export with `calamine` and compare every cell
with the engine's value, bit for bit for doubles. Excel can't run on the build
host, so `cargo run -p parallax-export --example verification` writes a workbook
whose second and third columns must read the same in real Excel on a VDI.

## Deliberately out of scope

Authentication, entitlements and audit belong to the host application. Cell
editing, Python bindings and the other deferred items are listed with their
reasons in [Deferred work](future.md). The milestones, build order and
performance budgets are in the [Roadmap](roadmap.md).
