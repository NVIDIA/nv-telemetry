# nv-telemetry-redfish

`nv-telemetry-redfish` projects compiled `nv-redfish` schema types into
`nv-telemetry-core` observations. It performs no I/O and holds no state beyond
a signal catalog: callers fetch resources, hand them here, and receive rows the
core model already validates.

## Crate structure

```text
src/
├── lib.rs           public API
├── projection.rs    the projection trait, issues, and the declaration macro
├── sensor.rs        Sensor projections and the subject convention
├── signal.rs        signal identity, catalog, and sample resolution
└── uri.rs           reducing Redfish URIs to the resource they denote
```

## Declaring a projection

`telemetry_projection!` declares a mapping from one source type to one output.
Required fields are all evaluated, so a failed projection reports every missing
or invalid field rather than only the first. Optional fields are evaluated only
once the required ones hold. A type mismatch between a source field and the
output fails at compile time.

A field is *missing* when the device said nothing and *invalid* when it
answered unusably, a distinction a consumer needs: a `NaN` reading is a device
reporting garbage, not a device staying quiet.

## Two layers per sensor

A Sensor read yields three separate things, because they change at different
rates and mean different things:

- `SensorMetadataProjection` produces the definition — metric, instance, kind,
  unit, and the range the part can read. It is refreshed rarely.
- `SensorSampleProjection` produces the value, joined to its definition by the
  catalog.
- `SensorResourceProjection` produces observed configuration, thresholds above
  all, as an `ObservedResource` for the resource graph.

Thresholds are not signal metadata. They are writable on most implementations,
which makes them a convergence target rather than a property of what a signal
*is*. A consumer joins them to a reading through the subject both carry.

That resource is `Partial`: it lifts a chosen subset of the representation, so
absence proves nothing. Because absence cannot speak, a device that reports the
`Thresholds` object gets all six thresholds, with the ones it leaves unset
stated as null. A device that omits the object gets none, which is the separate
claim that it does not implement them.

## Identity

A subject names what a thing is; the URI it was read from stays the resource's
source key. `Sensor.Id` is unique only within one chassis' collection, so a
sensor's subject is `sensor` / `{chassis}/{id}`, with the chassis taken from
the URI because the payload never states it. A location that yields no chassis
is reported as an invalid field rather than guessed at, since a wrong subject
joins silently to the wrong resource.

`SignalKey` is the identity a signal has across acquisition routes. Each route
spells it differently — a metric report names a reading with a property
fragment that the sensor resource does not use — so every conversion reduces
its URI to the resource denoted. That happens when the key is built rather than
at each call site, because a key that skipped it would fail to join without
failing loudly.

## Acquisition routes

Only the Sensor route can define a signal. An EnvironmentMetrics excerpt
carries a reading and a `DataSourceUri`, and a metric report carries a value
and a `MetricProperty`; neither carries units or a reading type. Both are
cheaper ways to refresh a value that the Sensor route has already defined, and
`SignalCatalog::resolve` reports `UnresolvedSignal` for a sample whose
definition has not been read yet.

## Catalog revisions

Readings share a descriptor by pointer, so the catalog owns revisions. A
refresh reporting an unchanged definition keeps the existing descriptor, which
makes a revision count real changes rather than polls and lets sharing survive
a refresh. Confirmation time is tracked beside the descriptor rather than on
it, so re-observing a signal does not replace what readings already hold.
