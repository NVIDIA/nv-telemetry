# Architecture

## Purpose

`nv-telemetry` is a read-only telemetry collection library for bare-metal
systems. It accepts endpoint inventory and collection intent, plans how to
obtain the requested data, executes collection through an embedder-owned
dispatcher, and emits immutable observations.

The library reports what it observed. It does not know why the data is needed
or what a larger system intends to do with it.

## Scope

The following decisions define the library boundary:

- **Sources only observe.** They may read sensors, logs, state, and inventory,
  but they never mutate a device.
- **Health is consumer policy.** Threshold classification, health verdicts,
  staleness decisions, rack aggregation, and remediation are outside the
  library.
- **Endpoint inventory comes from the embedder.** The library reacts to
  endpoint additions, changes, and removals. It does not discover endpoints.
- **Exporters are optional consumers.** OTLP and Prometheus support may be
  feature-gated, but they consume the same output available to any embedder.

## Ownership boundaries

The design has four planes:

```
┌─ Integration plane: embedding service ───────────────────────────────┐
│ endpoint inventory, collection intent, configuration, reconciliation │
│ loop, final dispatcher graph/runtime, consumers and output routing   │
├─ Orchestration plane: nv-telemetry ──────────────────────────────────┤
│ data needs, provider planning, capability probing, dispatcher task   │
│ vocabulary, typed policy, and standard subtree recipes               │
├─ Acquisition plane: nv-telemetry, feature-gated by protocol ─────────┤
│ Redfish/gRPC/REST/UDP providers that construct requests and parse    │
│ responses; no scheduling, sink, health, or mutation policy           │
├─ Data plane: nv-telemetry core ──────────────────────────────────────┤
│ immutable observation model, acquisition status, shared batches      │
└───────────────────────────────────────────────────────────────────────┘
```

The embedding service owns the application. The library provides reusable
planning, acquisition, and data-model components without hiding an application
runtime inside itself.

## End-to-end flow

```
Endpoint inventory ─┐
                    ├─> embedder reconciliation loop
Generic data needs ─┘             │
                                  v
    ┌─────────────────> library provider planner
    │                             │
    │                             v
    │                 dispatcher subtree recipes
    │         (acquisition tasks and capability probes)
    │                             │
    │                             v
    │              embedder-owned dispatcher graph
    │                             │
    │                             v
    │                  read-only source tasks
    │                             │
    │            ┌────────────────┼────────────────┐
    │            v                v                v
    │       observation      acquisition      capability
    │         batches          status          results
    │            └───────┬────────┘                │
    │                    v                         │
    │            consumer adapters                 │
    └──────────────────────────────────────────────┘
                 replan when capability is learned
```

On an endpoint or policy change, the embedder invokes the planner and recipes
to update only the affected portion of its dispatcher graph. Endpoint removal,
task cancellation, draining, and final graph lifecycle remain under embedder
control.

The loop on the left is why planning is not a single pass: a capability probe
is itself dispatched work, so an unknown capability resolves only after its
probe has been admitted and run.

## Inputs

The library accepts three kinds of generic input.

### Endpoint events

An endpoint event adds, changes, or removes an endpoint. An endpoint definition
contains:

- a stable endpoint identity;
- one or more supported access methods;
- an opaque credential reference or credential provider;
- static attributes such as rack, serial number, and device class;
- a generation or equivalent revision used to detect changes.

Secrets must not enter observation batches, diagnostics, or logs.

### Data needs

A data need describes what the deployment wants to observe, not which protocol
request to execute. Examples include:

- sensor readings every 30 seconds;
- firmware inventory every hour;
- device state observations;
- log records or an event subscription.

A need may carry cadence, freshness, timeout, and priority requirements. It
must not encode a specific provider unless the embedder intentionally applies a
provider override.

### Collection policy

Collection policy configures planning, dispatch, and delivery independently of
source implementation. Configuration changes are reconciled in the same way as
endpoint changes and do not require source-specific code.

## Needs, providers, and planning

A **data need** states what data is required. A **provider** states one way to
obtain it.

Three kinds of discovery remain distinct:

- **Endpoint inventory** is supplied by the embedder and determines which
  devices exist.
- **Capability probing** determines which providers an endpoint supports.
- **Resource cataloging** discovers protocol resources inside an endpoint,
  such as sensors, log services, or firmware components.

All three feed planning, but they differ in who produces them. Endpoint
inventory is a control input: it arrives from the embedder and the library
never derives it. A capability result is a control fact the library produces
itself by issuing a probe request. A resource catalog is an acquisition result
that can be reused by other acquisition stages and published as observation
data.

Collapsing any pair moves work into the wrong plane. If the library derived
endpoint inventory, it would decide which devices exist, which belongs to the
embedder. If capability probing and resource cataloging were one step, learning
whether a bulk provider exists would require walking every sensor, making
planning as expensive as collection and coupling plan stability to device data
churn; the two also age differently, since capability is near-static and
TTL-cached while a catalog changes whenever hardware does. If cataloging
happened inside planning, the planner would perform I/O and the catalog could
not also be emitted as an inventory batch.

For example, sensor readings could be provided by:

- Redfish TelemetryService with one bulk request;
- Redfish Sensor OData with multiple widely-supported requests;
- a switch-specific REST API.

Each provider declares:

- the need it can satisfy;
- its provider identity and request class;
- whether it is polled or streamed;
- its estimated request cost;
- how to probe endpoint capability;
- how to construct requests and parse responses.

The planner evaluates providers per endpoint. It caches capability results with
a configurable TTL, walks a deterministic preference list, and selects the
first supported provider. Persistent provider-local failure may demote that
provider and cause replanning.

This is intentionally not a scoring engine. Selection must remain explainable:
the resolved plan records the selected provider and why alternatives were
rejected or demoted.

### Probing is dispatched work

A capability probe is a request against a device, so it is subject to the same
admission control as any acquisition: it runs as a dispatcher task under the
endpoint's concurrency limits, rate budget, and breakers, and carries its own
request class. Nothing in the library issues an unscheduled request. Without
this, a fleet-wide restart or a wave of TTL expiries would probe every endpoint
at once, which is exactly the burst the dispatcher exists to smooth, and probes
would evade the breaker that an unreachable endpoint has already tripped.

Planning is therefore asynchronous rather than a single blocking pass. When a
need has no cached capability result, the planner emits a probe task and leaves
that need unresolved; the probe result lands as a control fact and triggers a
replan for the affected endpoint. A resolved plan is consequently allowed to be
partial, and the embedder reconciles its dispatcher graph incrementally as
capability becomes known.

An endpoint whose capability is still unknown produces no observations and no
fabricated ones. Probe failures are reported as acquisition status like any
other request failure.

### Composable acquisition stages

A provider recipe expands a data need into a directed acyclic graph of
acquisition stages. A stage declares its prerequisites and may produce:

- a private, protocol-specific artifact for downstream stages;
- one or more public observation batches, but only for requested data needs.

Equivalent stages for the same endpoint are deduplicated. Their artifacts are
cached by generation or TTL and invalidated when endpoint access changes or
the provider reports that they are stale.

For Redfish sensors, a `SensorCatalog` stage discovers resources once and
retains `SensorLink` handles privately. The same catalog can then:

- project sensor metadata into an inventory batch when sensor inventory was
  explicitly requested;
- expand into per-sensor OData reading tasks;
- provide metadata needed to interpret bulk TelemetryService readings.

If both inventory and readings are requested, they share one catalog fetch. If
only readings are requested, the catalog remains an internal prerequisite and
does not publish unsolicited inventory.

Logs, firmware, and other domains use their own catalog or probe stages. They
share only prerequisites they explicitly declare, such as a Redfish
`ServiceRoot` stage:

```
ServiceRoot
  ├─ SensorCatalog ──> sensor inventory and/or reading tasks
  ├─ LogServiceCatalog ──> periodic log tasks
  ├─ EventServiceProbe ──> SSE subscription
  └─ FirmwareCatalog ──> firmware inventory
```

The embedder chooses data needs. Protocol modules declare recipes. The planner
resolves provider preferences and deduplicates stages. Every stage that
performs network I/O is admitted by the dispatcher.

Private artifacts such as `SensorLink` never enter the data plane. Public
inventory items and readings instead share a stable, protocol-neutral subject
identity so consumers can correlate them without understanding Redfish.

### Typed projection

Protocol integrations map compiled source types into the data plane through
compile-checked projection declarations. A declaration names required source
fields, optional derived values, and the output constructor. Missing or invalid
required fields produce structured projection issues; Rust type mismatches fail
at compile time.

Signal metadata and samples are projected separately. Metadata produces an
immutable `SignalDescriptor` indexed by a canonical `SignalKey`. Sensor,
EnvironmentMetrics, and MetricReport samples project to the same key and are
joined with the descriptor through a `SignalCatalog`. This avoids repeating
units and bounds on every sample while allowing the planner to change
acquisition routes without changing public reading identity.

Because readings share a descriptor by pointer, the catalog owns metadata
revisions. A refresh that reports an unchanged definition keeps the existing
descriptor, so revision counts real definition changes rather than polls and
sharing survives across refreshes.

A descriptor carries only what a signal *is*: its identity, kind, unit, and the
range the sensor can read. Device-settable configuration, alarm thresholds
being the obvious case, is observed as resource state instead. The line is
mutability. A threshold is writable on most implementations, which makes it a
convergence target rather than reading metadata, and carrying it in both places
would give one fact two representations that can disagree. Consumers that
classify readings join by subject against the resource graph.

Non-finite source values are dropped during projection and surface as a missing
field. The observation model admits only finite floats, so that observations
remain exactly comparable and can survive a serialization round trip.

Projection declarations contain source-specific field knowledge. The core
observation model and dispatcher do not depend on Redfish schema types.

## Dispatcher placement

The dispatcher is the execution mechanism between planning and acquisition:

- the planner decides **what** should be collected and **which provider**
  should supply it;
- library recipes translate that plan into dispatcher subtrees;
- the dispatcher decides **when** polled work may run;
- the admitted source task constructs the request and parses the response.

Capability probes are dispatched on the same terms. Planning emits them as
tasks rather than issuing them directly, so the question the planner needs
answered is subject to the same limits, breakers, and request-class policy as
the collection it enables.

The library exposes dispatcher-compatible task types, typed tuning policy, and
standard subtree recipes. The embedding service owns the final graph, graph
reconciliation, node lifecycle, and the `Runtime` driving loop. This allows
telemetry work to participate in a larger application dispatcher graph.

Sources do not implement their own scheduling, retries, backoff, rate limiting,
or sinks. Every task carries a request-class identity so those behaviors can be
configured consistently outside protocol code.

### Standard dispatcher recipe

The standard recipe is opinionated but not mandatory:

```
global concurrency
  └─ endpoint admission
      └─ endpoint connectivity breaker
          └─ request-class lane
              └─ request-class breaker
                  └─ rate/burst token bucket
                      └─ priority queue
                          └─ task leaves
```

The two breaker scopes serve different purposes:

- common connectivity, endpoint availability, or authentication failures may
  affect the endpoint breaker;
- unsupported or failing provider-specific requests affect only their request
  class.

The exact dispatcher node composition is an implementation contract with the
dispatcher library. The architecture requires the scopes and behavior above,
not a particular internal node representation.

## Configuration

Configuration is layered and deterministically resolved. Later, more-specific
values override earlier values:

1. library defaults;
2. deployment defaults;
3. protocol overrides;
4. endpoint-selector overrides, such as device class or rack;
5. individual endpoint overrides;
6. request-class overrides.

The resolved configuration and its contributing layers must be inspectable for
every planned task.

Policy is divided into four areas:

- **Collection intent:** enabled needs, cadence, freshness, timeout, and
  priority.
- **Provider policy:** preference order, capability-probe TTL, and failure
  demotion.
- **Dispatcher policy:** global and per-endpoint concurrency, token-bucket
  rate and burst, priority lane, endpoint and request-class breakers,
  retry/backoff, jitter, and shutdown behavior.
- **Delivery policy:** per-consumer queue bounds and overflow behavior.

The library supports three levels of customization:

1. use defaults and specify only data needs;
2. override typed policy values while using the standard recipes;
3. bypass recipes and compose raw dispatcher nodes.

The third level is the escape hatch for applications whose topology cannot be
expressed by the standard recipes. The library does not define a configuration
language for arbitrary graph construction.

Configuration validation rejects impossible rates, unbounded queues, unknown
request classes, invalid endpoint selectors, and retry policies that cannot
meet their collection freshness requirements.

## Acquisition contract

A source is deliberately small. It:

1. builds a request or subscription from endpoint access;
2. parses protocol data into the shared observation model;
3. classifies acquisition failures sufficiently for dispatcher and planner
   policy.

It does not choose cadence, retry itself, publish to a sink, derive health, or
update external state.

One acquisition may produce more than one homogeneous observation batch. For
example, a response containing readings and inventory may emit one batch for
each domain while retaining common endpoint, origin, and timing metadata.

## Output contract

The library exposes two generic output streams.

### Observation batches

`Arc<ObservationBatch>` is the sole telemetry unit of flow. A batch contains:

- shared endpoint context;
- origin and provider/request-class provenance;
- the observation time or window;
- completeness information;
- one homogeneous payload domain: readings, logs, state observations,
  inventory, or an observed resource graph.

Batches are immutable and safe to fan out by reference. Consumers may include
exporters, in-process channels, persistence, APIs, or external adapters.

Completeness is part of the observation, not an acquisition error:

- a complete snapshot may be used to infer that previously known inventory is
  absent;
- a partial snapshot must not silently delete unobserved inventory;
- a failed request emits no fabricated device observation.

### Observed resource graph

The resource graph is the structured snapshot domain for OOB device state. It
preserves canonical subjects, original source keys, source schemas and
versions, recursive properties, explicit nulls, unresolved references, and
typed directed relationships.

Graph snapshots may represent an entire endpoint or a subtree of one. Subject
scope on a graph means reachability from the scope subject rather than subject
equality, because a graph is a connected structure and the natural unit of
partial collection is a subtree such as one chassis and its contents. A
complete graph can establish absence for resources and relationships within
that scope. A partial graph updates observed facts but cannot establish that
omitted facts were removed.

Reachability follows relations from source to target. A subtree therefore has
to hang off its root by outgoing edges, which is a constraint on projections: a
walk that records only the inverse relation, each child naming its parent,
produces a graph its own root cannot reach.

Absence is also tracked per resource. A resource records whether its properties
are the device's full representation, so a consumer can tell a property the
device does not implement from one that was never requested. A convergence
consumer that conflates the two would read an uncollected property as unset and
attempt to write it.

Each resource maps to exactly one source location. Data from another URI becomes
its own resource joined by a relation, never a merge, because merging collapses
two observation times and two entity tags into one.

Resource targets may be outside the collected graph so partial Redfish and
switch topology walks can retain external links. Relationship sources must be
present because the source is the resource on which the relation was observed.
Cycles are valid: containment, management, fabric, and peer relationships do
not form one universal tree.

Both ends of a relationship are identities, so an edge is a resolved claim
rather than a collected one. Because targets may be external, stating one costs
no fetch, only knowledge of what the target is called; where a source names
things canonically that comes out of the link itself. A link a walk cannot yet
name stays an unresolved reference on the resource, carrying the location with
no identity attached, and becomes an edge when a later pass resolves it. The
distinction is what keeps a partial walk honest, since an identity invented to
force an edge is indistinguishable from a real external target.

Graphs are stored in canonical sorted order and carry no non-finite floats, so
a whole graph can be compared and content-hashed. A convergence adapter depends
on that: state comparison must be exact and must not report spurious change.

Graph size and property nesting are bounded at the model boundary, so no
malformed endpoint response becomes a stored graph that later exhausts a
collector serving many endpoints. The bound is on what the model accepts, not
on what a source may buffer before offering it: an acquisition reading an
endpoint incrementally owns its own ceiling, and collection policy owns
request-level limits. A caller may tighten the bound but not loosen it, because
decoding has no caller to take one from and applies the default: a graph built
past it would encode into a payload the model then refuses to read, and the
model should not be able to produce what it cannot consume.

The graph is suitable input for an embedder-owned adapter that materializes
convergence observed state. Desired state, drift calculation, operation
selection, and mutation remain outside `nv-telemetry`.

### Acquisition status

`AcquisitionStatus` reports operational outcomes such as success or failure,
retryability, duration, endpoint, provider, and request class. It allows the
embedder to operate the collection pipeline without confusing pipeline failure
with device state.

Observation absence, collection failure, and stale prior data are distinct.
The library preserves the information needed for consumers to apply their own
freshness and absence policies.

### Delivery

The embedder owns fan-out and delivery. Consumer queues must be bounded and
their overflow behavior explicit. A slow consumer must not retain unbounded
history or silently impose application-wide backpressure.

The library guarantees ordering only at a documented local scope, such as an
endpoint/provider task; it does not imply global ordering across endpoints.

## Optional integrations

Feature-gated OTLP and Prometheus exporters consume observation batches like
any other adapter. Export mapping and serialization occur only at the exporter
boundary.

## Data-model requirements

The data plane defines semantic vocabulary, not application policy. It should
have no I/O dependencies and should keep protocol, dispatcher, exporter, and
convergence concepts out of core observation types.

The model must preserve:

- endpoint identity without copying static context into every item;
- recursive typed properties, explicit nulls, references, units, timestamps,
  attributes, and provenance;
- stable resource identity and typed relationships across protocol sources;
- complete versus partial snapshot semantics;
- immutable sharing across multiple consumers;
- private construction invariants and room to change internal storage.

The initial public contract must not depend on a specific memory layout.

## Modularity

Prefer a small number of crates with strong Cargo feature boundaries:

- core observation and orchestration vocabulary stays lightweight;
- protocol/domain sources are optional features such as Redfish sensors,
  Redfish logs, REST, and gRPC;
- exporters are optional features;
- heavy dependencies are pulled only by the features that require them.

## Open decisions

### Streamed acquisition placement

Polled work maps directly to dispatcher tasks. Streamed sources such as
Redfish SSE or gRPC subscriptions still require a concrete design choice:

- represent stream reads with an adapter inside the dispatcher graph, keeping
  more activity under common fairness control; or
- run subscriptions beside the dispatcher while admitting connection and
  reconnection attempts through it.

The source abstraction must preserve both options until one real streamed
source is prototyped. The decision must account for cancellation, reconnect
backoff, fairness, shutdown, and output backpressure.

### Batch storage layout

The semantic batch API should be defined before selecting an optimized storage
layout. Benchmark a simple owned representation against table-based layouts
using realistic payloads and multiple consumers. Adopt additional complexity
only where measurements show material benefit.

## Design validation

Before freezing public APIs, validate the architecture with:

1. one polled need implemented by two providers, such as Redfish
   TelemetryService and Sensor OData;
2. one streamed provider;
3. endpoint and policy changes that rebuild only affected graph subtrees;
4. complete, partial, failed, stale, and slow-consumer scenarios;
5. representative allocation, memory-retention, and throughput measurements.
