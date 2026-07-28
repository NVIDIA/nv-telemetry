# nv-telemetry-core

`nv-telemetry-core` is the source-neutral data plane for `nv-telemetry`:
immutable observations and acquisition status, with no protocol, network
client, async runtime, dispatcher, exporter, or health policy. Protocol crates
translate their responses into these types, so applications route the same
observations to storage, APIs, or health processing without touching
protocol-specific objects.

## Data flow

1. An acquisition source collects protocol data.
2. The source projects each result into a core row type.
3. Rows from one domain are placed in a homogeneous `Payload`.
4. The payload is wrapped in an `ObservationBatch` with endpoint, provenance,
   time, and coverage.
5. The immutable batch is shared as `Arc<ObservationBatch>`.
6. Success or failure is reported separately as `AcquisitionStatus`.

A failure never produces a synthetic observation. An empty complete batch, an
empty partial batch, and a failed acquisition all mean different things.

## Crate structure

```text
src/
├── lib.rs                 public API and top-level documentation
├── status.rs              acquisition outcomes and failure classification
└── model/
    ├── mod.rs             model exports and completeness semantics
    ├── name.rs            owned or static names
    ├── number.rs          finite floating-point observation values
    ├── attributes.rs      sorted typed key/value attributes
    ├── time.rs            timestamps and observation windows
    ├── context.rs         endpoints, subjects, origin, scope, coverage
    ├── reading.rs         signal metadata and numeric readings
    ├── log.rs             log records and severity
    ├── state.rs           source-reported state observations
    ├── inventory.rs       discovered inventory items
    └── batch.rs           payloads, batches, validation, row builders
```

Modules are implementation boundaries; common types are re-exported from the
crate root.

## Observation batch

`ObservationBatch` is the canonical unit of telemetry flow:

- `Arc<EndpointContext>`: endpoint identity and attributes;
- `Origin`: provider and request-class provenance;
- `ObservationWindow`: when the source observed the data;
- `Coverage`: the scope and completeness claim;
- `Payload`: exactly one observation domain.

`Payload` is `#[non_exhaustive]` with four variants — `Readings`, `Logs`,
`States`, `Inventory` — each holding a `Box<[_]>` of rows.

A batch and everything in it implements `Eq` and `Hash`, so a snapshot works as
a cache key and a consumer can tell a changed observation from a repeated one
without walking it field by field. Batches are homogeneous; one acquisition may
emit several when its response spans domains. `ReadingsBuilder`, `LogsBuilder`,
`StatesBuilder`, and `InventoryBuilder` accumulate rows, and `finish` converts
them into immutable boxed slices.

## Shared ownership

`EndpointContext` is behind an `Arc` because one is reattached to every batch
and status an endpoint produces, and `SignalDescriptor` because one is shared by
every sample of its signal. `Arc::ptr_eq` is a fast path for "same definition",
not a replacement for comparing descriptors: readings built from different
catalogs hold equal definitions separately.

Constructors take `impl Into<Arc<_>>`, so a projection passes the handle it
holds and a one-off caller passes the value. The batch itself is not forced
into shared ownership: `ObservationBatch::new` returns it by value, and
`SharedBatch` is the alias for when several consumers need the same one.

Serde does not carry `Arc` identity, so `Payload::Readings` encodes as a
descriptor table with rows indexing into it. Entries match by value, so equal
payloads encode to identical bytes and decoding can only widen sharing. A
`Reading` serialized alone carries its descriptor inline.

## Identity and provenance

- `EndpointId` identifies the managed endpoint.
- `Subject` identifies the device, component, or sensor being observed.
- A row's source key identifies the original protocol record.
- `Origin` identifies the provider and request class that produced the batch.

These must not be collapsed into one string: a subject can be observed through
several providers, and one provider can report many source records for the same
subject.

## Readings and signal metadata

`SignalDescriptor` holds the long-lived metadata — subject, metric and instance
identity, reading kind and unit, observation time and revision, attributes,
thresholds and bounds — and each `Reading` adds its source key, numeric value,
sample timestamp, sample attributes, and source-reported state. The split lets
different acquisition routes, such as an individual sensor request and a bulk
metric report, produce readings with one logical signal identity.
`NumericValue` preserves signed integers, unsigned integers, and floats rather
than forcing everything through `f64`.

`SignalDescriptor::matches_definition` compares everything except observation
time and revision, so a catalog can tell a real change from a repeated poll and
leave the descriptor in place. `revision` therefore counts definition changes
rather than refreshes, and `observed_at` marks when the current definition first
appeared; a catalog tracks re-observation separately so it can still prune
signals an endpoint has stopped reporting.

`Thresholds` and `ReadingBounds` record what a device reported, including
contradictory lanes such as an upper critical below an upper caution. Their
`checked` methods report an inversion; projections drop an inconsistent set
rather than publishing it.

## Finite values

Floating-point observations use `Finite`, which rejects `NaN` and the
infinities at construction. `NaN` compares unequal to itself, so one non-finite
value would make an observation look changed on every diff and would bar `Eq`
and `Hash` entirely.

An unmeasurable quantity is absence, not a sentinel float: omit the reading.
Projections drop non-finite source values. Use the `finite!` macro for
literals, which fails at compile time.

## Scope and completeness

Completeness is always relative to `ObservationScope`:

- `Endpoint` covers the declared endpoint, `Subject(subject)` covers one subject.
- `Complete` says every observation in that scope was obtained.
- `Partial` says absence must not be inferred for omitted rows.

In a subject-scoped batch every row that names a subject must match the
declared one, and `ObservationBatch::new` rejects mismatches with
`BatchError::SubjectOutsideScope`. Logs and states may omit a subject; such a
row is an endpoint-level observation admissible under any scope. Readings and
inventory items always name one and are always checked.

A complete snapshot establishes absence only inside its declared scope.
Staleness is not completeness: consumers apply freshness policy using
observation timestamps.

## Acquisition status

`AcquisitionStatus` is an operational record, not device telemetry. It carries
the same endpoint, origin, and window as a batch, with an `AcquisitionOutcome`
of `Succeeded { emitted_batches }` or `Failed { class, retryable }`.
`FailureClass` gives stable categories — transport, timeout, authentication,
authorization, rate limiting, unsupported data, invalid response — so a
dispatcher can act on a failure without it entering the payload.

## Invariants

- `Attributes` sorts keys and rejects duplicates.
- `Timestamp` validates nanoseconds.
- `ObservationWindow` rejects an end before its start.
- `ObservationBatch` validates subject-scoped payloads.
- `Finite` rejects `NaN` and the infinities.

With the `serde` feature, these types deserialize through their constructors
rather than the derived field-by-field path, so a value arriving over the wire
is held to the same rules as one built in process.

## Field visibility

A field is private when it carries a rule, so the private fields of a struct
are exactly the ones its constructor cross-checks. The test is per field, not
per type: `ObservationBatch` keeps `coverage` and `payload` private because
`new` validates them against each other, while `endpoint`, `origin`, and
`window` are public. `Attributes`, `Timestamp`, `ObservationWindow`, `Finite`,
and newtypes such as `Name` are private throughout.

Everything else is a plain record with public fields — `Subject`, `Origin`,
`Coverage`, `Reading`, `SignalDescriptor`, `LogRecord`, and the rest. Read them
as `reading.value`, match with `Subject { kind, .. }`, and build them with the
`new` and `with_*` methods. They are `#[non_exhaustive]`, so a struct literal
will not compile outside this crate and a new field is not a breaking change.
`Thresholds` and `ReadingBounds` are deliberately here: they record what a
device reported, and `checked` is the opt-in way to reject a contradictory
range.

## Example

```rust
use nv_telemetry_core::{
    finite, Attributes, Coverage, EndpointContext, ObservationBatch, ObservationWindow,
    Origin, Payload, Reading, ReadingKind, ReadingsBuilder, SignalDescriptor, Subject,
    Timestamp, Unit,
};

fn build_batch() -> Result<ObservationBatch, Box<dyn std::error::Error>> {
    let observed_at = Timestamp::new(1_700_000_000, 0)?;
    let subject = Subject::new("sensor", "CPU0Temp");
    let descriptor = SignalDescriptor::new(
        subject.clone(),
        "temperature",
        "CPU0Temp",
        ReadingKind::Gauge,
        Unit::from_static("Cel"),
        observed_at,
    );

    let mut rows = ReadingsBuilder::new();
    rows.push(
        Reading::new("/redfish/v1/Chassis/1/Sensors/CPU0Temp", descriptor, finite!(42.5))
            .with_observed_at(observed_at),
    );

    Ok(ObservationBatch::new(
        EndpointContext::new("bmc-1", Attributes::empty()),
        Origin::new("redfish-sensor", "sensor-reading"),
        ObservationWindow::point(observed_at),
        Coverage::complete_subject(subject),
        Payload::Readings(rows.finish()),
    )?)
}
```

## Features

The default build is `std`-only with no serialization dependency. The optional
`serde` feature adds serialization and validated deserialization.

```toml
[dependencies]
nv-telemetry-core = { version = "0.1", features = ["serde"] }
```

## Development

```sh
cargo test -p nv-telemetry-core --all-features
cargo doc -p nv-telemetry-core --all-features --no-deps
```

`make all` runs formatting, Clippy, builds, tests, doctests, and documentation
checks for the whole workspace.
