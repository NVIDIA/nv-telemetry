# nv-telemetry-redfish

`nv-telemetry-redfish` projects compiled `nv-redfish` schema types into
`nv-telemetry-core` observations. It performs no I/O. Callers own fetching,
scheduling, endpoint identity, and batch assembly.

## Crate structure

```text
src/
├── lib.rs           public API and crate documentation
├── projection.rs    the projection trait, field values, and issue reporting
├── sensor.rs        Sensor metadata, sample, and resource projections
├── signal.rs        signal identity, catalog, and sample resolution
└── uri.rs           URI canonicalization and parent-scope extraction
```

## Projection contract

`Project<Input, Context>` is the compile-time projection interface.
Implementations are normally zero-sized marker types. A projection offers every
required source field to `Fields::require` before deciding whether it can
build, so one failure reports all required issues. `Fields::optional` records
invalid optional values without blocking valid output; a missing optional field
is not an issue. Finalization is total: `complete` discards output after a
required failure, and an inconsistent `incomplete` call reports
`InvalidProjection` instead of panicking.

```rust
use nv_telemetry_redfish::{FieldValue, Fields, Project, ProjectionResult};

struct Source {
    value: Option<u64>,
}

struct Double;

impl Project<Source> for Double {
    type Output = u64;

    fn project(source: &Source, _context: &()) -> ProjectionResult<u64> {
        let mut fields = Fields::new();
        let Some(value) =
            fields.require("Source.Value", FieldValue::from_option(source.value))
        else {
            return fields.incomplete();
        };
        fields.complete(value * 2)
    }
}

assert_eq!(Double::project(&Source { value: Some(21) }, &()).output(), Some(&42));
```

A field is *missing* when the device said nothing and *invalid* when it
answered unusably, a distinction a consumer needs: a `NaN` reading is a device
reporting garbage, not a device staying quiet.

`Project` has an associated function and is not a trait-object interface.
Runtime selection uses a typed function pointer such as `P::project`, or an
application-owned object-safe adapter around that function. This keeps source
and output types checked while allowing dispatch tables.

## Sensor outputs

A Sensor response has three projections because its facts change at different
rates:

- `SensorMetadataProjection` produces the definition — metric, instance, kind,
  unit, and valid readable range.
- `SensorSampleProjection` produces the value and preserves `ReadingTime` as
  the reading timestamp. Poll time remains the batch/resource observation time.
- `SensorResourceProjection` produces partial observed configuration and
  preserves `@odata.etag` in `ObservedResource::version`.

The resource projection returns `SensorResourceRecord`, a graph fragment that
keeps the Sensor resource and its parent-to-child `contains` relation together.
The caller adds the parent resource when assembling a parent-scoped
`ResourceGraph`; keeping the edge with the child avoids an unreachable graph.

Thresholds are resource configuration, not signal metadata. All ten Redfish
slots are represented:

- upper/lower caution, critical, and fatal;
- upper/lower caution-user and critical-user.

Each present slot is a nested object containing `reading`, `activation`,
`dwell_time`, `hysteresis_reading`, and `hysteresis_duration`. Missing fields
inside a present slot are explicit nulls. Invalid optional values are reported
as issues and omitted; they do not discard the resource or manufacture a null.
If the whole `Thresholds` object is absent, no threshold properties are emitted.

## Identity and URI handling

A subject names the thing observed; the canonical endpoint-local URI remains
its source key. `Sensor.Id` is unique only within its parent collection, so the
subject includes parent scope:

- Chassis: `sensor / chassis/{chassis-id}/{sensor-id}`;
- PowerDistribution:
  `sensor / power_distribution/{collection}/{parent-id}/{sensor-id}`.

Supported PowerDistribution parent collections are FloorPDUs, RackPDUs,
Switchgear, TransferSwitches, PowerShelves, and ElectricalBuses. The emitted
parent subject uses kind `power_distribution` and retains the normalized
collection in its ID. A Sensor outside a supported parent collection is
rejected instead of being assigned a guessed identity.

Absolute and relative URIs, authorities, query strings, fragments, and trailing
slashes canonicalize to the same endpoint-local path. `SignalKey` applies this
normalization at construction so Sensor, EnvironmentMetrics, and MetricReport
routes can join.

## Signal catalog

Only a full Sensor response defines signal metadata. Other routes produce a
`SignalSample`, which `SignalCatalog::resolve` joins to the descriptor.

A catalog is endpoint-local: keys deliberately omit host identity, so never
share one catalog between BMCs. Growth is bounded by
`SignalCatalog::DEFAULT_MAX_SIGNALS` or a caller-provided limit. Iteration,
removal, and `retain_confirmed_since` support disappearance handling.

`upsert` returns `Added`, `Revised`, `Unchanged`, or `Stale`. An older update,
or a conflicting definition at the same confirmation time, is stale and cannot
replace metadata or move confirmation time backward. An unchanged later update
keeps the existing `Arc<SignalDescriptor>` and advances confirmation time.

`UnresolvedSignal` retains the rejected sample and implements `Error`, allowing
the caller to buffer it and retry after metadata arrives.

## End-to-end example

```rust
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::{NumericValue, Timestamp};
use nv_telemetry_redfish::{
    Project, SensorMetadataProjection, SensorProjectionContext,
    SensorSampleProjection, SignalCatalog,
};

let sensor: Sensor = serde_json::from_str(r#"{
    "@odata.id": "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
    "Id": "CPU0Temp",
    "Name": "CPU 0 Temperature",
    "Reading": 42.5,
    "ReadingTime": "1970-01-01T00:00:01Z",
    "ReadingType": "Temperature",
    "ReadingUnits": "Cel",
    "ReadingRangeMin": -10.0,
    "ReadingRangeMax": 110.0
}"#)?;
let context = SensorProjectionContext::new(Timestamp::new(10, 0)?);

let metadata = SensorMetadataProjection::project(&sensor, &context)
    .into_parts().0.ok_or("missing metadata")?;
let sample = SensorSampleProjection::project(&sensor, &context)
    .into_parts().0.ok_or("missing sample")?;

let mut catalog = SignalCatalog::new();
catalog.upsert(metadata)?;
let reading = catalog.resolve(sample)?;

assert_eq!(reading.signal.subject.id.as_str(), "chassis/1/CPU0Temp");
assert_eq!(reading.value, NumericValue::f64(42.5)?);
assert_eq!(reading.observed_at, Some(Timestamp::new(1, 0)?));
# Ok::<(), Box<dyn std::error::Error>>(())
```

All Rust examples in this README are included in crate doctests.

`SignalCatalog::upsert` reports `SignalCatalogError::Full` when a new identity
exceeds the configured bound and `SignalCatalogError::RevisionExhausted` if an
installed signal has consumed the full `u64` revision space. Both errors retain
the rejected metadata record.

## Migration notes

Intentional source breaks:

- replace `telemetry_projection!` with a zero-sized type implementing
  `Project<Input, Context>`; feed source values through `FieldValue` and
  `Fields::{require, optional}`, then finish with `complete` or `incomplete`;
- construct `SensorProjectionContext` and pass it to each Sensor projection;
- consume `ProjectionResult` through its accessors or `into_parts`; direct
  construction is not public;
- use `Fields::optional` for invalid-but-nonblocking source fields;
- consume `SensorResourceRecord` as a resource/relation pair instead of
  treating the Sensor resource as a standalone graph;
- handle `SignalCatalog::upsert` capacity errors and the `SignalUpdate::Stale`
  outcome;
- recover unresolved samples with `UnresolvedSignal::into_sample`;
- use typed function pointers or adapters for runtime projection dispatch
  instead of `dyn Project`.

## Package policy

The crate has the workspace MSRV of Rust 1.89. Its path dependency on
`nv-telemetry-core` also carries a version, so package publication can resolve
the dependency outside this workspace.

Focused checks:

```console
cargo test -p nv-telemetry-redfish
cargo test -p nv-telemetry-redfish --doc
cargo doc -p nv-telemetry-redfish --no-deps --document-private-items
```
