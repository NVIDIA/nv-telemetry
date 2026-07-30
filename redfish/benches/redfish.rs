// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Instruction-count benchmarks for Redfish projection and signal lookup.
//!
//! Each setup deserializes the integration-test Sensor fixture and builds the
//! projection context before Callgrind starts, so measurements cover
//! projection or catalog work rather than JSON decoding or timestamp
//! validation. Inputs and outputs are returned to keep their drops outside the
//! measured region.

#![allow(unused_qualifications)]

#[cfg(unix)]
#[path = "../tests/common/mod.rs"]
mod common;

#[cfg(unix)]
mod unix {
    use std::hint::black_box;

    use gungraun::library_benchmark;
    use nv_redfish::schema::sensor::Sensor;
    use nv_telemetry_core::Reading;
    use nv_telemetry_redfish::Project;
    use nv_telemetry_redfish::ProjectionResult;
    use nv_telemetry_redfish::SensorMetadataProjection;
    use nv_telemetry_redfish::SensorProjectionContext;
    use nv_telemetry_redfish::SensorResourceProjection;
    use nv_telemetry_redfish::SensorResourceRecord;
    use nv_telemetry_redfish::SensorSampleProjection;
    use nv_telemetry_redfish::SignalCatalog;
    use nv_telemetry_redfish::SignalDescriptorRecord;
    use nv_telemetry_redfish::SignalSample;
    use nv_telemetry_redfish::SignalUpdate;

    use crate::common::context;
    use crate::common::fixture;
    use crate::common::project;

    fn projection_input() -> (Sensor, SensorProjectionContext) {
        (fixture(), context())
    }

    fn upsert_input() -> (SignalCatalog, SignalDescriptorRecord) {
        let sensor = fixture();
        (
            SignalCatalog::new(),
            project::<SensorMetadataProjection>(&sensor),
        )
    }

    fn resolve_input() -> (SignalCatalog, SignalSample) {
        let sensor = fixture();
        let mut catalog = SignalCatalog::new();
        catalog
            .upsert(project::<SensorMetadataProjection>(&sensor))
            .expect("catalog capacity");
        (catalog, project::<SensorSampleProjection>(&sensor))
    }

    #[library_benchmark]
    #[bench::fixture(projection_input())]
    fn sensor_metadata(
        (sensor, context): (Sensor, SensorProjectionContext),
    ) -> (Sensor, ProjectionResult<SignalDescriptorRecord>) {
        let result = black_box(SensorMetadataProjection::project(&sensor, &context));
        (sensor, result)
    }

    #[library_benchmark]
    #[bench::fixture(projection_input())]
    fn sensor_sample(
        (sensor, context): (Sensor, SensorProjectionContext),
    ) -> (Sensor, ProjectionResult<SignalSample>) {
        let result = black_box(SensorSampleProjection::project(&sensor, &context));
        (sensor, result)
    }

    #[library_benchmark]
    #[bench::fixture(projection_input())]
    fn sensor_resource(
        (sensor, context): (Sensor, SensorProjectionContext),
    ) -> (Sensor, ProjectionResult<SensorResourceRecord>) {
        let result = black_box(SensorResourceProjection::project(&sensor, &context));
        (sensor, result)
    }

    #[library_benchmark]
    #[bench::added(upsert_input())]
    fn catalog_upsert(
        (mut catalog, record): (SignalCatalog, SignalDescriptorRecord),
    ) -> (SignalCatalog, SignalUpdate) {
        let update = black_box(catalog.upsert(record)).expect("catalog capacity");
        (catalog, update)
    }

    #[library_benchmark]
    #[bench::hit(resolve_input())]
    fn catalog_resolve(
        (catalog, sample): (SignalCatalog, SignalSample),
    ) -> (SignalCatalog, Reading) {
        let reading = black_box(catalog.resolve(sample)).expect("catalogued fixture metadata");
        (catalog, reading)
    }
}

#[cfg(unix)]
use unix::catalog_resolve;
#[cfg(unix)]
use unix::catalog_upsert;
#[cfg(unix)]
use unix::sensor_metadata;
#[cfg(unix)]
use unix::sensor_resource;
#[cfg(unix)]
use unix::sensor_sample;

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = sensor_projections;
    benchmarks = sensor_metadata, sensor_sample, sensor_resource
);

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = signal_catalog;
    benchmarks = catalog_upsert, catalog_resolve
);

#[cfg(unix)]
gungraun::main!(
    library_benchmark_groups = sensor_projections,
    signal_catalog
);

#[cfg(not(unix))]
fn main() {}
