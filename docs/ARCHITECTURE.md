# Architecture

## Current scope

This repository currently contains two libraries:

- `nv-telemetry-core`: the source-neutral immutable observation model and
  acquisition status;
- `nv-telemetry-redfish`: typed, I/O-free projections from `nv-redfish` Sensor
  values into core observations.

There is no planner, network client, scheduler, dispatcher, exporter, or
application runtime in this workspace. Embedders own endpoint inventory,
credentials, requests, retries, scheduling, persistence, fan-out, freshness,
health policy, and mutation.

## Data flow

```text
embedder fetches typed source data
              |
              v
protocol projection + structured issues
        |                    |
        v                    v
signal metadata ------> endpoint-local catalog
        |                    |
        +---------- sample --+
                             v
                          Reading

resource projection ------> resource + relation fragment
                             |
                             v
                       ResourceGraph

rows/graph + endpoint + origin + window + coverage
                             |
                             v
                     ObservationBatch
```

An acquisition may emit several homogeneous batches. Operational failure is an
`AcquisitionStatus`, not a synthetic observation.

## Core model

Compound identities use typed components:

- endpoint identity: `EndpointId`;
- observed identity: `Subject { SubjectKind, SubjectId }`;
- source location: `SourceKey`;
- provenance: `Origin { Provider, RequestClass }`;
- signal identity: `Metric` at an `Instance`.

Invariant-bearing values are valid at construction. This includes finite
numbers, timestamps, observation windows, property maps, resource graphs, and
`ValueRange`. A closed range is created with `ValueRange::between`; one-sided
ranges use `at_least` or `at_most`.

Rows are immutable and homogeneous by payload. Row order is part of equality
and hashing, including logs; core does not infer that a row domain is
unordered. Producers that use hashes for change detection must choose a
deterministic source order.

Staleness and completeness are independent. Completeness controls whether
omission establishes absence inside the declared scope. Freshness is consumer
policy based on timestamps.

## Resource graph

`ResourceGraph` is assembled directly from resource and relation vectors.
There is no graph builder. Assembly:

- sorts resources by subject and relations by `(source, kind, target)`;
- rejects duplicate subjects, source keys, and relation identities;
- requires every relation source to be present;
- permits external relation targets;
- applies bounded graph and property-depth limits.

Recursive arrays are constructed through `PropertyArray`, so the depth bound
holds before a `PropertyValue` can reach derived clone, equality, hash,
serialization, or drop implementations.

A relation is an explicit resolved fact. An unresolved protocol link remains a
`PropertyValue::Reference` until its target identity is known.

Subject-scoped graph validation uses directed reachability, not equality.
`ResourceGraph::reachability_from` distinguishes a missing root, complete
reachability, and the first unreachable resource. A non-empty subtree therefore
needs outgoing parent-to-child relations. Cycles are valid.

An `ObservedResource` corresponds to one source location. Its optional version
can carry concurrency metadata such as an ETag. Combining two source locations
would lose separate observation times and versions, so they remain separate
resources joined by relations.

## Projection boundary

Redfish projections are written against compiled schema types. `Fields`
collects all required failures and reports invalid optional fields without
discarding otherwise valid output. Finalization never panics on source data:
required failures suppress output, while inconsistent projection control flow
is returned as an `InvalidProjection` issue.

`Project<Input, Context>` is static and generic. Runtime dispatch uses typed
function pointers such as `P::project`, or an embedder-owned object-safe
adapter. The trait itself is not a runtime trait-object interface.

Sensor projections split one response into:

- metadata used to define a signal;
- a sample joined to metadata through `SignalCatalog`;
- partial resource state and a parent `contains` relation.

This separation keeps writable threshold configuration out of signal
definitions while allowing a consumer to join both by subject.

## Redfish Sensor semantics

Sensor identity is scoped by the parent collection encoded in the URI.
Chassis and all supported PowerDistribution collections produce distinct
subjects even when `Sensor.Id` repeats. Absolute/relative URIs, query strings,
fragments, and trailing separators canonicalize to one endpoint-local key.

`ReadingTime` is the sample timestamp. Poll time is the projection context and
belongs to metadata/resource observation and the eventual batch window.
`@odata.etag` is retained as the resource version.

All ten Redfish threshold slots are resource properties. A present slot is a
nested object containing reading, activation, dwell time, hysteresis reading,
and hysteresis duration. A missing field in a present slot is explicit null; an
invalid optional value is an issue and is omitted. If `Thresholds` is absent,
the partial resource makes no threshold claim.

`SensorResourceRecord` keeps the projected Sensor and its parent relation
together. The parent resource remains caller-owned, but the fragment contains
the edge needed for subject-scope reachability.

## Signal catalog

`SignalCatalog` is local to one endpoint because normalized keys contain no
host identity. It has an explicit entry bound and supports iteration, removal,
and confirmation-based retention.

Catalog updates cannot move confirmation time backward. Older metadata and
conflicting metadata at the same time return `SignalUpdate::Stale` without
replacing the installed definition. Unchanged later metadata preserves the
shared descriptor while advancing confirmation time.

An unresolved sample is returned in `UnresolvedSignal`, allowing the embedder
to buffer and retry it after metadata arrives.

## Serialization and packaging

Core's serde representation uses transparent semantic wrappers and boolean
`Retryable` values. Deserialization applies the model's constructor checks, so
an inverted `ValueRange` is rejected. Readings use a descriptor table on the
wire and restore shared descriptors when decoded. Serde does not provide
schema negotiation; integrations that need it must supply an external
schema/version boundary.

The workspace MSRV is Rust 1.89. Both packages inherit it, docs.rs builds all
features, and the Redfish package gives its path dependency a publishable
version. As a library workspace, the repository does not commit `Cargo.lock`;
development, CI, and benchmarks resolve the semver ranges that consumers see.

## Deliberate non-goals

- network transport and credential handling;
- provider selection and capability probing;
- scheduling, retries, rate limiting, or backpressure;
- exporters and persistence;
- threshold classification, health verdicts, or freshness policy;
- desired state, drift calculation, and mutation.
