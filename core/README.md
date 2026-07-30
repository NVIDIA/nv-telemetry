# nv-telemetry-core

`nv-telemetry-core` is the source-neutral data plane for `nv-telemetry`. It
contains immutable observations and acquisition status, with no protocol,
network client, async runtime, dispatcher, exporter, or health policy.

## Data model

An acquisition emits one or more homogeneous `ObservationBatch` values. Each
batch carries:

- shared endpoint identity and attributes;
- typed provider and request-class provenance;
- an `ObservationWindow`;
- scope-relative `Coverage`;
- exactly one `Payload` domain.

Failures are emitted separately as `AcquisitionStatus`; they never manufacture
device observations. Empty complete, empty partial, failed, and stale data are
distinct states.

The implementation is split into `status.rs` and these model modules:

```text
attributes  batch  collection  context  inventory  log  name
number      property  reading  resource  state      time
```

Common types are re-exported from the crate root.

## Typed construction

Compound identities use distinct component types so swapped arguments do not
compile:

- `Subject::new(SubjectKind, SubjectId)`;
- `Origin::new(Provider, RequestClass)`;
- `SignalDescriptor::new(Subject, Metric, Instance, ReadingKind, Unit, Timestamp)`;
- `ResourceRelation::new(Subject, RelationKind, Subject)`.

String vocabulary types accept owned or static strings through `From` and
`from_static`. Invariant-bearing types use checked constructors:

- `Finite::new` rejects `NaN` and infinities;
- `Timestamp::new` and `DurationValue::new` validate nanoseconds;
- `ObservationWindow::new` rejects reversed windows and
  `checked_duration` derives a checked span;
- `ValueRange::empty`, `at_least`, and `at_most` are infallible, while
  `ValueRange::between` rejects inverted bounds;
- `PropertyValue::array` rejects recursive values beyond the property-depth
  limit;
- `ResourceGraph::new` and `with_limits` validate complete graph input.

There is no public resource-graph builder. Collect resources and relations,
then assemble them directly with `ResourceGraph`.

## Identity, ordering, and sharing

`EndpointId` identifies the managed endpoint, `Subject` identifies the thing
observed, `SourceKey` records its protocol location, and `Origin` records how it
was acquired. These identities are intentionally separate.

Row order is part of the identity and hash of `Readings`, `Logs`, `States`, and
`Inventory` payloads. Core does not sort those domains because their source
order may be meaningful. Producers using batch hashes for change detection
must emit rows in a deterministic source-defined order.

Resource graphs are different: resources sort by subject and relations by
`(source, kind, target)`, so discovery order is not graph identity.

`EndpointContext`, signal descriptors, attributes, and property maps use
immutable shared storage. Pointer equality is only a fast path; equality and
hashing remain content-based. With serde enabled, a readings payload stores a
descriptor table and rows index into it, preserving sharing without putting
pointer identity on the wire.

## Readings

`SignalDescriptor` contains stable metadata: subject, metric and instance,
reading kind, unit, definition observation time and revision, attributes, and
an optional valid `ValueRange`. `Reading` adds the source key, numeric value,
optional source sample time, attributes, and source-reported state.

A per-reading timestamp may predate the batch window. It records when the
device sampled the value; the batch window records when acquisition ran.

`SignalDescriptor::matches_definition` ignores `observed_at` and `revision`.
Catalogs can therefore keep the shared descriptor for an unchanged refresh,
increment revisions only for definition changes, and track confirmation time
separately.

Thresholds are observed configuration, not signal metadata. They belong in a
resource graph and join to readings through the shared subject.

## Resource graphs

An `ObservedResource` represents one source location and carries its subject,
source key, completeness, optional schema/version, observation time, and
recursive `PropertyMap`. `PropertyValue` preserves explicit nulls, references,
bytes, timestamps, and durations. `PropertyArray` makes recursive array
construction checked, so property depth is bounded before values can be
cloned, hashed, serialized, or dropped. Graph size is bounded separately.

`ResourceCompleteness::Complete` means an omitted property is absent;
`Partial` means omission proves nothing. A relation is a resolved directed fact
whose source must be present. Its target may be outside the collected graph.
An unresolved link remains a `PropertyValue::Reference`.

Subject-scoped row payloads require matching subjects. Subject-scoped graphs
instead require explicit reachability from the scope root by outgoing
relations. `ResourceGraph::reachability_from` distinguishes:

- `Reachability::MissingRoot`;
- `Reachability::FullyReachable`;
- `Reachability::Unreachable(subject)`.

An empty graph is valid under any scope. A non-empty graph must contain its
scope root and every collected resource must be reachable from it. Projections
that model containment should emit parent-to-child `contains` relations rather
than only inverse edges.

## Serde compatibility policy

The optional `serde` feature preserves the existing representation of values
accepted by current model invariants. Semantic string wrappers remain
transparent, `Retryable::{Yes, No}` remain JSON/CBOR booleans, and
invariant-bearing values deserialize through the same checks used by
constructors. A legacy `ValueRange` with `lower > upper` is therefore
intentionally rejected.

Wire evolution is strict:

- do not send observation fields or enum variants to a reader that does not
  know them;
- an integration receiving mixed-version data must reject an unknown
  observation shape at its schema/version boundary, never deserialize and
  silently re-emit a value with newer fields removed;
- adding a required field, changing a field meaning, or changing a wire shape
  requires an explicit compatibility plan and normally a major version;
- accepted legacy JSON and CBOR fixtures, and intentional legacy rejections,
  are kept as compatibility tests.

Serde support validates known core values; it is not a version-negotiating
envelope. Systems that persist or exchange observations should carry an
external schema/version discriminator and validate it before decoding.

## Example

```rust
use nv_telemetry_core::{
    Attributes, Coverage, EndpointContext, Finite, Instance, Metric,
    ObservationBatch, ObservationWindow, Origin, Payload, Provider, Reading,
    ReadingKind, RequestClass, SignalDescriptor, SourceKey, Subject, SubjectId,
    SubjectKind, Timestamp, Unit,
};

fn build_batch() -> Result<ObservationBatch, Box<dyn std::error::Error>> {
    let observed_at = Timestamp::new(1_700_000_000, 0)?;
    let subject = Subject::new(
        SubjectKind::from_static("sensor"),
        SubjectId::from_static("chassis/1/CPU0Temp"),
    );
    let descriptor = SignalDescriptor::new(
        subject.clone(),
        Metric::from_static("temperature"),
        Instance::from_static("CPU0Temp"),
        ReadingKind::Gauge,
        Unit::from_static("Cel"),
        observed_at,
    );
    let reading = Reading::new(
        SourceKey::from_static("/redfish/v1/Chassis/1/Sensors/CPU0Temp"),
        descriptor,
        Finite::new(42.5)?,
    )
    .with_observed_at(observed_at);

    ObservationBatch::new(
        EndpointContext::new("bmc-1", Attributes::empty()),
        Origin::new(
            Provider::from_static("redfish-sensor"),
            RequestClass::from_static("sensor-reading"),
        ),
        ObservationWindow::point(observed_at),
        Coverage::complete_subject(subject),
        Payload::Readings(vec![reading].into_boxed_slice()),
    )
    .map_err(Into::into)
}
```

## Migration from the pre-0.1 API

Intentional source breaks:

- pass typed components to `Subject`, `Origin`, and `SignalDescriptor`
  constructors instead of interchangeable strings;
- replace `finite!(value)` with `Finite::new(value)` and handle its
  `Result`;
- replace `GraphLimits::new(resources, relations)` with
  `GraphLimits::default().with_max_resources(resources)
  .with_max_relations(relations)`;
- replace reads of `NonFiniteError::value` with matches on its `NaN`,
  `PositiveInfinity`, and `NegativeInfinity` classifications;
- replace `resource.establishes_absence()` with
  `resource.completeness.establishes_absence()`;
- replace removed row and resource macros/builders with constructors,
  `with_*` methods, `Vec::into_boxed_slice`, and `ResourceGraph::new`;
- replace unchecked or post-hoc range validation with the named `ValueRange`
  constructors;
- replace direct `PropertyValue::Array(values.into_boxed_slice())` construction
  with `PropertyValue::array(values)?`; the wire representation remains an
  ordinary array;
- replace ambiguous unreachable-resource queries with
  `reachability_from` and match all `Reachability` outcomes;
- replace `outcome.retryable()` with `outcome.is_retryable()`, and pass
  `Retryable::Yes` or `Retryable::No` to `AcquisitionOutcome::failed`; its
  wire form remains a boolean.

## Package policy

The minimum supported Rust version is Rust 1.89, declared by the workspace and
checked in CI. The workspace lockfile is committed for reproducible
development, CI, and instruction-count benchmarks; published libraries still
use normal semver dependency ranges.

The default build has no serialization dependency. Enable serde with:

```toml
[dependencies]
nv-telemetry-core = { version = "0.1", features = ["serde"] }
```

Focused checks:

```console
cargo test -p nv-telemetry-core --all-features
cargo test -p nv-telemetry-core --doc --all-features
cargo doc -p nv-telemetry-core --all-features --no-deps --document-private-items
```
