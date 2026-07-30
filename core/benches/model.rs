// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Instruction-count benchmarks for the observation model (gungraun /
//! Valgrind Callgrind).
//!
//! These cover what an endpoint poll costs: a projection builds attributes
//! and readings for every sample, and the batch it hands over is validated
//! and often hashed. Names and formatted keys are built in setup and passed
//! in, so a measurement reports model work rather than `format!`, and each
//! benchmark returns what it built so the drop stays outside. Valgrind is
//! unix-only, so the whole benchmark is `cfg(unix)`.

// The gungraun attribute macros re-emit each setup expression with fully
// qualified paths, which the workspace's `unused_qualifications` lint counts
// against the call site. Nothing here is written qualified by hand.
#![allow(unused_qualifications)]

#[cfg(unix)]
mod unix {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::hint::black_box;
    use std::sync::Arc;
    use std::time::SystemTime;

    use gungraun::library_benchmark;
    use nv_telemetry_core::Attribute;
    use nv_telemetry_core::Attributes;
    use nv_telemetry_core::Coverage;
    use nv_telemetry_core::EndpointContext;
    use nv_telemetry_core::Finite;
    use nv_telemetry_core::Name;
    use nv_telemetry_core::ObservationBatch;
    use nv_telemetry_core::ObservationWindow;
    use nv_telemetry_core::Origin;
    use nv_telemetry_core::Payload;
    use nv_telemetry_core::Reading;
    use nv_telemetry_core::ReadingKind;
    use nv_telemetry_core::SignalDescriptor;
    use nv_telemetry_core::SourceKey;
    use nv_telemetry_core::Subject;
    use nv_telemetry_core::Timestamp;
    use nv_telemetry_core::Unit;

    /// A chassis worth of sensors, the unit an endpoint poll produces.
    const SENSORS: usize = 256;

    /// Attributes per row: endpoint labels plus a few source dimensions.
    const LABELS: usize = 8;

    /// A row's inputs, pre-built so a benchmark measures only assembly.
    type Row = (SourceKey, Arc<SignalDescriptor>);

    fn timestamp() -> Timestamp {
        Timestamp::new(1_700_000_000, 0).expect("valid timestamp")
    }

    fn subject(index: usize) -> Subject {
        Subject::new("sensor".into(), format!("CPU{index}Temp").into())
    }

    /// Labels in the order a projection emits them, which is not sorted:
    /// `Attributes::new` pays for ordering them.
    fn labels(count: usize) -> Vec<Attribute> {
        (0..count)
            .map(|index| Attribute::new(format!("label_{:02}", count - index), format!("v{index}")))
            .collect()
    }

    fn attributes(count: usize) -> Attributes {
        Attributes::new(labels(count)).expect("unique label keys")
    }

    fn descriptor(subject: Subject) -> Arc<SignalDescriptor> {
        let instance = Name::from(subject.id.as_str());
        Arc::new(SignalDescriptor::new(
            subject,
            "temperature".into(),
            instance.into(),
            ReadingKind::Gauge,
            Unit::from_static("Cel"),
            timestamp(),
        ))
    }

    /// One poll's rows, each with its own signal, as a sensor walk yields.
    fn distinct_rows(count: usize) -> (Vec<Row>, Attributes) {
        let rows = (0..count)
            .map(|index| {
                (
                    format!("/redfish/v1/Chassis/1/Sensors/CPU{index}Temp").into(),
                    descriptor(subject(index)),
                )
            })
            .collect();
        (rows, attributes(LABELS))
    }

    /// Rows that all describe one subject, which is what a subject-scoped
    /// batch has to hold for validation to accept it.
    fn single_subject_rows(count: usize) -> (Vec<Row>, Attributes) {
        let signal = descriptor(subject(0));
        let rows = (0..count)
            .map(|index| {
                (
                    format!("/redfish/v1/Chassis/1/Sensors/CPU0Temp/{index}").into(),
                    Arc::clone(&signal),
                )
            })
            .collect();
        (rows, attributes(LABELS))
    }

    fn assemble(rows: Vec<Row>, attributes: &Attributes) -> Box<[Reading]> {
        rows.into_iter()
            .map(|(source_key, signal)| {
                Reading::new(source_key, signal, Finite::new(42.5).unwrap())
                    .with_observed_at(timestamp())
                    .with_attributes(attributes.clone())
            })
            .collect()
    }

    fn endpoint() -> Arc<EndpointContext> {
        Arc::new(EndpointContext::new("bmc-1", attributes(LABELS)))
    }

    /// A batch and the parts needed to build another exactly like it, so a
    /// benchmark can measure assembly alone.
    fn batch_input(
        rows: (Vec<Row>, Attributes),
        coverage: Coverage,
    ) -> (Box<[Reading]>, Arc<EndpointContext>, Coverage) {
        let (rows, attributes) = rows;
        (assemble(rows, &attributes), endpoint(), coverage)
    }

    fn batch(rows: (Vec<Row>, Attributes)) -> ObservationBatch {
        let (payload, endpoint, coverage) = batch_input(rows, Coverage::complete_endpoint());
        ObservationBatch::new(
            endpoint,
            Origin::new("redfish-sensor".into(), "sensor-reading".into()),
            ObservationWindow::point(timestamp()),
            coverage,
            Payload::Readings(payload),
        )
        .expect("valid batch")
    }

    // Sorting and duplicate-rejecting one row's labels.
    #[library_benchmark]
    #[bench::labels_8(labels(8))]
    #[bench::labels_32(labels(32))]
    fn attributes_new(labels: Vec<Attribute>) -> Attributes {
        black_box(Attributes::new(labels)).expect("unique label keys")
    }

    // Binary search against the sorted key slice.
    #[library_benchmark]
    #[bench::hit((attributes(LABELS), "label_01".to_owned()))]
    #[bench::miss((attributes(LABELS), "absent".to_owned()))]
    fn attributes_get((attributes, key): (Attributes, String)) -> Attributes {
        black_box(attributes.get(&key));
        attributes
    }

    // Static vocabulary is allocation-free; device-supplied text is not.
    #[library_benchmark]
    #[bench::from_static()]
    fn name_from_static() -> Name {
        black_box(Name::from_static("temperature"))
    }

    #[library_benchmark]
    #[bench::from_owned("CPU0Temp".to_owned())]
    fn name_from_owned(value: String) -> Name {
        black_box(Name::from(value))
    }

    // Epoch conversion, which a projection performs per timestamped row.
    #[library_benchmark]
    #[bench::now(SystemTime::now())]
    fn timestamp_from_system_time(value: SystemTime) -> Timestamp {
        black_box(Timestamp::from_system_time(value)).expect("representable timestamp")
    }

    #[library_benchmark]
    #[bench::epoch(timestamp())]
    fn timestamp_to_system_time(value: Timestamp) -> SystemTime {
        black_box(value.to_system_time()).expect("representable system time")
    }

    // Turning a poll's pre-built rows into a readings payload. Attributes
    // are cloned per row, as a projection reattaching endpoint labels does.
    #[library_benchmark]
    #[bench::sensors_256(distinct_rows(SENSORS))]
    fn build_readings((rows, attributes): (Vec<Row>, Attributes)) -> Box<[Reading]> {
        black_box(assemble(rows, &attributes))
    }

    // Batch assembly, which is where scope validation happens. Both cases
    // carry the same row count, so the difference between them is the
    // subject scan an endpoint scope skips.
    #[library_benchmark]
    #[bench::endpoint_scope(batch_input(distinct_rows(SENSORS), Coverage::complete_endpoint()))]
    #[bench::subject_scope(batch_input(
        single_subject_rows(SENSORS),
        Coverage::complete_subject(subject(0))
    ))]
    fn build_batch(
        (payload, endpoint, coverage): (Box<[Reading]>, Arc<EndpointContext>, Coverage),
    ) -> ObservationBatch {
        black_box(ObservationBatch::new(
            endpoint,
            Origin::new("redfish-sensor".into(), "sensor-reading".into()),
            ObservationWindow::point(timestamp()),
            coverage,
            Payload::Readings(payload),
        ))
        .expect("valid batch")
    }

    // Content-addressing a snapshot, the operation that lets a consumer tell
    // a changed batch from a repeated one without walking it. The fixture's
    // row order is deterministic because row order is part of batch identity.
    #[library_benchmark]
    #[bench::sensors_256(batch(distinct_rows(SENSORS)))]
    fn hash_batch(batch: ObservationBatch) -> ObservationBatch {
        let mut hasher = DefaultHasher::new();
        batch.hash(&mut hasher);
        black_box(hasher.finish());
        batch
    }

    // Equality over a whole snapshot, the other half of change detection.
    // Equal batches are the worst case: comparison cannot exit early.
    #[library_benchmark]
    #[bench::sensors_256((batch(distinct_rows(SENSORS)), batch(distinct_rows(SENSORS))))]
    fn compare_batches(
        pair: (ObservationBatch, ObservationBatch),
    ) -> (ObservationBatch, ObservationBatch) {
        black_box(pair.0 == pair.1);
        pair
    }
}

#[cfg(unix)]
use unix::attributes_get;
#[cfg(unix)]
use unix::attributes_new;
#[cfg(unix)]
use unix::build_batch;
#[cfg(unix)]
use unix::build_readings;
#[cfg(unix)]
use unix::compare_batches;
#[cfg(unix)]
use unix::hash_batch;
#[cfg(unix)]
use unix::name_from_owned;
#[cfg(unix)]
use unix::name_from_static;
#[cfg(unix)]
use unix::timestamp_from_system_time;
#[cfg(unix)]
use unix::timestamp_to_system_time;

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = attributes;
    benchmarks = attributes_new, attributes_get
);

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = primitives;
    benchmarks = name_from_static, name_from_owned, timestamp_from_system_time, timestamp_to_system_time
);

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = batches;
    benchmarks = build_readings, build_batch, hash_batch, compare_batches
);

#[cfg(unix)]
gungraun::main!(library_benchmark_groups = attributes, primitives, batches);

#[cfg(not(unix))]
fn main() {}
