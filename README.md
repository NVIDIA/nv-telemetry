# nv-telemetry
Rust libraries for collecting and modeling bare-metal telemetry.

## Crates

- [`nv-telemetry-core`](core/README.md) provides the source-neutral observation
  model and optional serde support.
- [`nv-telemetry-redfish`](redfish/README.md) projects Redfish resources into
  the core model.

## Compatibility

The minimum supported Rust version (MSRV) is Rust 1.89. The workspace lockfile
is checked in so development, CI, and instruction-count benchmarks resolve the
same dependency versions.

## Development

Run the workspace checks with:

```console
make all
```

Formatting uses the pinned nightly toolchain configured by the Makefile; the
libraries themselves support the MSRV above.

## Contribution guidelines

- Start here: [CONTRIBUTING.md](CONTRIBUTING.md)
- Code of Conduct: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)

## Security

- Vulnerability disclosure: [SECURITY.md](SECURITY.md)
- Do not file public issues for security reports.

## License

This project is licensed under the [Apache License 2.0](LICENSE).
