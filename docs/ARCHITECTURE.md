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
                         library provider planner
                                  │
                                  v
                      dispatcher subtree recipes
                                  │
                                  v
                    embedder-owned dispatcher graph
                                  │
                                  v
                         read-only source tasks
                                  │
                         ┌────────┴────────┐
                         v                 v
                 observation batches  acquisition status
                         └────────┬────────┘
                                  v
                          consumer adapters
```

On an endpoint or policy change, the embedder invokes the planner and recipes
to update only the affected portion of its dispatcher graph. Endpoint removal,
task cancellation, draining, and final graph lifecycle remain under embedder
control.

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

## Dispatcher placement

The dispatcher is the execution mechanism between planning and acquisition:

- the planner decides **what** should be collected and **which provider**
  should supply it;
- library recipes translate that plan into dispatcher subtrees;
- the dispatcher decides **when** polled work may run;
- the admitted source task constructs the request and parses the response.

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
- one homogeneous payload domain: readings, logs, state observations, or
  inventory.

Batches are immutable and safe to fan out by reference. Consumers may include
exporters, in-process channels, persistence, APIs, or external adapters.

Completeness is part of the observation, not an acquisition error:

- a complete snapshot may be used to infer that previously known inventory is
  absent;
- a partial snapshot must not silently delete unobserved inventory;
- a failed request emits no fabricated device observation.

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
- typed values, units, timestamps, attributes, and provenance;
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
