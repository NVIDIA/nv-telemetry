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

//! Instruction-count benchmarks for serialization (gungraun / Valgrind
//! Callgrind).
//!
//! `Payload::Readings` does not encode row by row: descriptors are hoisted
//! into a table and rows index into it, so encoding deduplicates and
//! decoding rebuilds the sharing. Both halves are measured against a
//! payload where every row shares one descriptor and one where each row
//! has its own, since the table is what separates those two costs.
//! Decoding also runs the batch's validation, which is deliberately inside
//! the measurement: it is not optional on the wire path.
//!
//! Valgrind is unix-only, so the whole benchmark is `cfg(unix)`.

// The gungraun attribute macros re-emit each setup expression with fully
// qualified paths, which the workspace's `unused_qualifications` lint counts
// against the call site. Nothing here is written qualified by hand.
#![allow(unused_qualifications)]

#[cfg(unix)]
mod unix {
    use std::hint::black_box;
    use std::sync::Arc;

    use gungraun::library_benchmark;
    use nv_telemetry_core::finite;
    use nv_telemetry_core::Attribute;
    use nv_telemetry_core::Attributes;
    use nv_telemetry_core::Coverage;
    use nv_telemetry_core::EndpointContext;
    use nv_telemetry_core::ObservationBatch;
    use nv_telemetry_core::ObservationWindow;
    use nv_telemetry_core::Origin;
    use nv_telemetry_core::Payload;
    use nv_telemetry_core::Reading;
    use nv_telemetry_core::ReadingKind;
    use nv_telemetry_core::ReadingsBuilder;
    use nv_telemetry_core::SignalDescriptor;
    use nv_telemetry_core::Subject;
    use nv_telemetry_core::Timestamp;
    use nv_telemetry_core::Unit;

    const SENSORS: usize = 256;
    const LABELS: usize = 8;

    fn timestamp() -> Timestamp {
        Timestamp::new(1_700_000_000, 0).expect("valid timestamp")
    }

    fn attributes(count: usize) -> Attributes {
        let labels = (0..count)
            .map(|index| Attribute::new(format!("label_{index:02}"), format!("v{index}")))
            .collect();
        Attributes::new(labels).expect("unique label keys")
    }

    fn descriptor(index: usize) -> SignalDescriptor {
        SignalDescriptor::new(
            Subject::new("sensor", format!("CPU{index}Temp")),
            "temperature",
            format!("CPU{index}Temp"),
            ReadingKind::Gauge,
            Unit::from_static("Cel"),
            timestamp(),
        )
    }

    /// A metric report: many samples of one signal, so the descriptor
    /// table holds a single entry however many rows there are.
    fn shared_descriptor(count: usize) -> Box<[Reading]> {
        let signal = Arc::new(descriptor(0));
        rows(count, |index| (index, Arc::clone(&signal)))
    }

    /// A sensor walk: every row is its own signal, so the table is as
    /// long as the payload and hoisting buys nothing.
    fn distinct_descriptors(count: usize) -> Box<[Reading]> {
        rows(count, |index| (index, Arc::new(descriptor(index))))
    }

    fn rows(
        count: usize,
        signal: impl Fn(usize) -> (usize, Arc<SignalDescriptor>),
    ) -> Box<[Reading]> {
        let attributes = attributes(LABELS);
        let mut rows = ReadingsBuilder::new();
        for index in 0..count {
            let (index, signal) = signal(index);
            rows.push(
                Reading::new(
                    format!("/redfish/v1/Chassis/1/Sensors/CPU{index}Temp"),
                    signal,
                    finite!(42.5),
                )
                .with_observed_at(timestamp())
                .with_attributes(attributes.clone()),
            );
        }
        rows.finish()
    }

    fn batch(rows: Box<[Reading]>) -> ObservationBatch {
        ObservationBatch::new(
            Arc::new(EndpointContext::new("bmc-1", attributes(LABELS))),
            Origin::new("redfish-sensor", "sensor-reading"),
            ObservationWindow::point(timestamp()),
            Coverage::complete_endpoint(),
            Payload::Readings(rows),
        )
        .expect("valid batch")
    }

    fn encoded(rows: Box<[Reading]>) -> String {
        serde_json::to_string(&batch(rows)).expect("serialize batch")
    }

    #[library_benchmark]
    #[bench::shared_signal(batch(shared_descriptor(SENSORS)))]
    #[bench::distinct_signals(batch(distinct_descriptors(SENSORS)))]
    fn encode(batch: ObservationBatch) -> (ObservationBatch, String) {
        let json = serde_json::to_string(&batch).expect("serialize batch");
        (batch, black_box(json))
    }

    #[library_benchmark]
    #[bench::shared_signal(encoded(shared_descriptor(SENSORS)))]
    #[bench::distinct_signals(encoded(distinct_descriptors(SENSORS)))]
    fn decode(json: String) -> (String, ObservationBatch) {
        let batch: ObservationBatch =
            serde_json::from_str(black_box(&json)).expect("deserialize batch");
        (json, black_box(batch))
    }
}

#[cfg(unix)]
use unix::decode;
#[cfg(unix)]
use unix::encode;

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = readings_table;
    benchmarks = encode, decode
);

#[cfg(unix)]
gungraun::main!(library_benchmark_groups = readings_table);

#[cfg(not(unix))]
fn main() {}
