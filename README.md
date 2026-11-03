# parallax

parallax is a data grid, pivot table and charting stack for portfolio managers
and quant researchers. It is a Rust engine with an Angular front end, built to
load and pivot tens of millions of rows on GPU-less Citrix desktops without
showing a wrong number.

parallax is in milestone 1 of its [roadmap](docs/roadmap.md), and there is
nothing to install yet. The synthetic data generator, `parallax-datagen`, is the
first working piece; [Contributing](CONTRIBUTING.md) shows how to build it and
run the tests.

## Documentation

- [How parallax is designed](docs/design.md) explains the architecture and
  the correctness rules, and why each choice was made.
- [Roadmap](docs/roadmap.md) lists the milestones, the build order and the
  performance budgets.
- [Deferred work](docs/future.md) records what was left out, and why.
- [The grid experiment](docs/grid-experiment.md) records why the grid draws on
  a canvas, with the measurements.

## License

Apache License 2.0. See [LICENSE](LICENSE).
