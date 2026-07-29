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
    ├── collection.rs      shared machinery for the sorted collections
    ├── attributes.rs      sorted typed key/value attributes
    ├── time.rs            timestamps, durations, and observation windows
    ├── context.rs         endpoints, subjects, origin, scope, coverage
    ├── reading.rs         signal metadata and numeric readings
    ├── log.rs             log records and severity
    ├── state.rs           source-reported state observations
    ├── inventory.rs       discovered inventory items
    ├── property.rs        recursive resource property values
    ├── resource.rs        observed resources, relations, resource graph
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

`Payload` is `#[non_exhaustive]` with five variants. `Readings`, `Logs`,
`States`, and `Inventory` each hold a `Box<[_]>` of rows; `Resources` holds a
`ResourceGraph`, which is a connected structure rather than a list.

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

`Attributes` and `PropertyMap` share their entries for the same reason: one
projection labels every row it emits, so cloning is a refcount bump rather than
a copy. The sharing is an implementation detail, since both are immutable once
built and compare by content.

Each also caches a digest of its entries in that shared allocation, computed on
first use, because hashing a snapshot would otherwise walk every entry of every
row. The digest is seeded per process, so an endpoint that chooses its own
attribute keys cannot precompute entries that collide in a consumer's map.
Caching counts as interior mutability, so clippy's `mutable_key_type` fires on a
`HashMap` keyed by anything holding one; the digest is derived from the entries
equality already compares, so it cannot change what a key hashes to.

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
identity, reading kind and unit, observation time and revision, attributes, and
readable bounds — and each `Reading` adds its source key, numeric value,
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

A signal's `bounds` is a `ValueRange`: the span the sensor is able to read,
with either edge optional. It is a capability of the device, not a judgement
about the value, and `checked` reports a contradictory pair so a projection can
drop it rather than publish it.

Alarm thresholds are deliberately absent. Every other field here is something
the device *is*; a threshold is something it is *configured to*, and on most
Redfish implementations it is writable. That makes it observed configuration
rather than reading metadata, so it travels in the resource graph below, where
a convergence engine can diff it against a desired value. A consumer that
classifies readings joins the two by subject.

## Observed resource graph

Readings and logs are high-volume and narrow, so they get purpose-built rows.
Device configuration, capabilities, and topology are the opposite — low-volume
and arbitrarily shaped — and they go in `Payload::Resources` as a
`ResourceGraph`. This is the observed half of a convergence comparison, so it
preserves the source's own shape rather than flattening it.

An `ObservedResource` carries its `Subject`, the `source_key` it came from,
optional schema and version for concurrency control, an observation time, and a
`PropertyMap`. `PropertyValue` recurses through objects and arrays and keeps
nulls, references, bytes, timestamps, and durations distinct rather than
stringifying them. `PropertyMap::MAX_DEPTH` caps nesting at 32, so walking or
releasing a stored map cannot overflow the stack; a value that arrives deeper
than that is dismantled iteratively as it is rejected, rather than taken apart
by the recursive drop glue. The cap is set by what a decoder reads back rather
than by what a walk survives: adjacent tagging multiplies each level against a
decoder's own recursion limit, so a looser cap would let this crate encode
graphs that a consumer of the same crate cannot decode.

Absence is three different facts, and the model keeps them apart.
`PropertyValue::Null` is a reported null. A property missing from a `Complete`
resource is one the device does not implement. The same property missing from a
`Partial` resource means nothing at all — `ResourceCompleteness` is what makes
`establishes_absence` answerable.

`ResourceRelation` is a typed directed edge identified by
`(source, kind, target)`; properties describe an edge but do not distinguish
two of them. Its source must be in the graph, while its target may be external,
so a subtree keeps its links into scopes that were not collected. Assembly
rejects a repeated subject, a repeated `source_key` (two fetches describing one
location cannot be merged silently), a repeated edge, and an edge leaving an
unknown subject. Resources sort by subject and relations by their identity
triple, so two graphs with the same content hash identically no matter what
order discovery found them in.

Both ends of a relation are `Subject`s, so an edge states a resolved fact. That
is not the same as a collected one: the target may be external, so the collector
needs the target's *identity*, not a fetch of it. Where a source names things
canonically the identity comes out of the link — a Redfish path names its
collection and id — so an edge can be stated as soon as the link is seen. Where
it does not, the link is not yet an edge. It stays on the resource as a
`PropertyValue::Reference`, which holds the location and leaves its `subject`
empty, until a pass that learns the identity promotes it. That is the difference
the two forms carry: an edge is a claim about topology, a reference is a link
the source stated and the collector has not resolved. Inventing a subject to
force the first would produce one no resource matches, which the graph cannot
distinguish from a genuine external target.

`GraphLimits` bounds what a graph may hold. It is checked once the input is
assembled, so it caps the stored snapshot rather than what a source buffers on
the way there. Limits only tighten `GraphLimits::DEFAULT`: deserialization has
no caller to take a bound from, so it applies the default, and a looser one
would let the crate build a graph it encodes and then refuses to read. A bound
above the default is clamped, which keeps "whatever the model accepts, it can
read back" true rather than conditional.

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

A graph is scoped by reachability instead. Requiring every resource to carry
the scope subject would allow only single-resource graphs, and the natural unit
of partial collection is a subtree — one chassis and what it contains. So a
non-empty subject-scoped graph must hold the scope subject
(`BatchError::MissingScopeRoot`) and every resource in it must be reachable
from there by following relations (`BatchError::UnreachableFromScopeRoot`).
An empty graph is accepted under any scope, as an empty row payload is.

Reachability follows relations from source to target, so a subtree has to hang
off its root by outgoing edges. A projection that emits only `containedBy`
edges pointing at the parent produces a graph its own root cannot reach.

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
- `Timestamp` and `DurationValue` validate nanoseconds against one bound.
- `ObservationWindow` rejects an end before its start.
- `ObservationBatch` validates subject-scoped payloads.
- `Finite` rejects `NaN` and the infinities.
- `PropertyMap` rejects duplicate names and nesting past `MAX_DEPTH`.
- `ResourceGraph` rejects duplicate subjects, source keys, and edges, edges
  from unknown subjects, and input past its `GraphLimits`.

With the `serde` feature, these types deserialize through their constructors
rather than the derived field-by-field path, so a value arriving over the wire
is held to the same rules as one built in process.

## Field visibility

A field is private when it carries a rule, so the private fields of a struct
are exactly the ones its constructor cross-checks. The test is per field, not
per type: `ObservationBatch` keeps `coverage` and `payload` private because
`new` validates them against each other, while `endpoint`, `origin`, and
`window` are public. `Attributes`, `Timestamp`, `ObservationWindow`, `Finite`,
`PropertyMap`, `ResourceGraph`, `DurationValue`, and newtypes such as `Name`
are private throughout.

Everything else is a plain record with public fields — `Subject`, `Origin`,
`Coverage`, `Reading`, `SignalDescriptor`, `LogRecord`, `Property`,
`ObservedResource`, `ResourceRelation`, and the rest. Read them
as `reading.value`, match with `Subject { kind, .. }`, and build them with the
`new` and `with_*` methods. They are `#[non_exhaustive]`, so a struct literal
will not compile outside this crate and a new field is not a breaking change.

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
