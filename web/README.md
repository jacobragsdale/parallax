# parallax web

The Angular workspace: the `parallax` library in `projects/parallax` and the
demo app in `projects/demo`. It uses pnpm.

| Script                              | What it does                                                        |
| ----------------------------------- | ------------------------------------------------------------------- |
| `pnpm generate`                     | Regenerates the protobuf code from `../proto`                       |
| `pnpm build`                        | Builds the library and the demo                                     |
| `pnpm build:demo`                   | Builds the demo into `dist/demo`, which the reference server serves |
| `pnpm test`                         | Runs the unit tests                                                 |
| `pnpm lint` and `pnpm format:check` | Check the code                                                      |
| `pnpm e2e`                          | Runs the Playwright tests against the reference server              |
| `pnpm performance`                  | Measures scrolling with the GPU off and the CPU throttled           |

The end-to-end tests start `target/release/parallax-server` with
`examples/experiment.toml`, so build the server first with
`cargo build --release -p parallax-server` from the repository root.
