// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! End-to-end milestone: the real provider, the real recipe, the real
//! dispatcher runtime, a mocked device, and a virtual timeline. An
//! embedder-shaped test — protocol crate and orchestration meet here, not
//! in the orchestration crate's own tests.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use std::time::Duration;
use std::time::Instant;

use nv_redfish_bmc_mock::Bmc;
use nv_redfish_bmc_mock::Expect;
use nv_redfish_dispatcher::ClockConfig;
use nv_redfish_dispatcher::ManualClock;
use nv_redfish_dispatcher::Runtime;
use nv_redfish_dispatcher::RuntimeConfig;
use nv_redfish_dispatcher::RuntimeOutput;
use nv_telemetry_model::EndpointContext;
use nv_telemetry_model::FailureClass;
use nv_telemetry_model::Outcome;
use nv_telemetry_model::Timestamp;
use nv_telemetry_orchestration::endpoint_subtree;
use nv_telemetry_orchestration::plan;
use nv_telemetry_orchestration::AcquisitionReport;
use nv_telemetry_orchestration::Clock;
use nv_telemetry_orchestration::EndpointFault;
use nv_telemetry_orchestration::EndpointPolicy;
use nv_telemetry_orchestration::PollMeta;
use nv_telemetry_orchestration::PollNeed;
use nv_telemetry_redfish::SensorRead;

const SENSOR: &str = "/redfish/v1/Chassis/1U/Sensors/CPU1Temp";
const FIXTURE: &str = include_str!("../fixtures/sensor.json");
const BASE_SECONDS: i64 = 1_785_621_243;

#[derive(Clone)]
struct TestClock {
    manual: ManualClock,
    epoch: Instant,
}

impl Clock for TestClock {
    fn timestamp(&self) -> Timestamp {
        let elapsed = self.manual.now().saturating_duration_since(self.epoch);
        let seconds = BASE_SECONDS + i64::try_from(elapsed.as_secs()).expect("a short test");
        Timestamp::new(seconds, elapsed.subsec_nanos()).expect("subsecond nanos are in bound")
    }

    fn instant(&self) -> Instant {
        self.manual.now()
    }
}

type PollRuntime = Runtime<AcquisitionReport, EndpointFault, PollMeta>;

fn describe(output: Option<&RuntimeOutput<AcquisitionReport, EndpointFault>>) -> &'static str {
    match output {
        None => "a parked runtime",
        Some(RuntimeOutput::Work { result: Ok(_), .. }) => "completed work",
        Some(RuntimeOutput::Work { result: Err(_), .. }) => "an endpoint fault",
        Some(RuntimeOutput::SleepUntil(_)) => "a sleep hint",
        Some(RuntimeOutput::Shutdown) => "shutdown",
        Some(RuntimeOutput::Runtime(_)) => "a runtime event",
    }
}

fn drive(runtime: &mut PollRuntime) -> Option<RuntimeOutput<AcquisitionReport, EndpointFault>> {
    let mut next = pin!(runtime.next());
    match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

#[test]
fn a_mocked_endpoint_polls_end_to_end() {
    let manual = ManualClock::new();
    let clock = TestClock {
        manual: manual.clone(),
        epoch: manual.now(),
    };

    let endpoint = EndpointContext::builder()
        .endpoint_id("bmc-lab-07")
        .build()
        .expect("a valid endpoint");
    let bmc = Arc::new(Bmc::<nv_redfish_bmc_mock::Error>::default());
    for _ in 0..3 {
        bmc.expect(Expect::get(SENSOR, FIXTURE));
    }

    let cadence = Duration::from_secs(30);
    let plan = plan(
        vec![PollNeed::new(endpoint.clone(), SENSOR, cadence)],
        &[SensorRead::<()>::declaration()],
    )
    .expect("the sensor declaration polls");
    let planned = plan.polls()[0].clone();
    let unit = Arc::new(SensorRead::new(
        endpoint.clone(),
        planned.target().to_owned().into(),
        Arc::clone(&bmc),
    ));

    let subtree = endpoint_subtree(&EndpointPolicy::default(), &clock, vec![(planned, unit)])
        .expect("one unit forms a subtree");
    let mut runtime: PollRuntime = Runtime::new(
        RuntimeConfig {
            global_max_in_flight: std::num::NonZeroUsize::MIN,
            clock: ClockConfig::Manual(manual.clone()),
        },
        subtree,
    );

    // Three primed ticks: each yields one report pairing a readings batch
    // with a success status under one identity and instant.
    for tick in 0..3 {
        let report = match drive(&mut runtime) {
            Some(RuntimeOutput::Work {
                result: Ok(mut reports),
                ..
            }) => reports.pop().expect("one acquisition, one report"),
            other => panic!(
                "tick {tick}: expected work, got {}",
                describe(other.as_ref())
            ),
        };
        assert_eq!(report.status().outcome(), Outcome::Succeeded);
        assert!(!report.batches().is_empty(), "the fixture yields readings");
        for batch in report.batches() {
            assert_eq!(batch.endpoint(), &endpoint);
            assert_eq!(batch.origin().provider(), SensorRead::<()>::PROVIDER);
            assert_eq!(batch.window().start(), report.status().started_at());
        }
        assert!(report.issues().is_none(), "the nominal fixture is clean");

        match drive(&mut runtime) {
            Some(RuntimeOutput::SleepUntil(deadline)) => manual.advance_to(deadline),
            other => panic!(
                "tick {tick}: expected the cadence hint, got {}",
                describe(other.as_ref())
            ),
        }
    }

    // The fourth tick finds the mock unprimed: a harness failure the
    // provider classifies as Internal — reported, request-scoped, and the
    // breaker untouched.
    let report = match drive(&mut runtime) {
        Some(RuntimeOutput::Work {
            result: Ok(mut reports),
            ..
        }) => reports.pop().expect("one acquisition, one report"),
        other => panic!("expected the failed tick, got {}", describe(other.as_ref())),
    };
    assert_eq!(report.status().outcome(), Outcome::Failed);
    assert_eq!(
        report.status().failure_class(),
        Some(FailureClass::Internal)
    );
    assert!(
        report.batches().is_empty(),
        "a failed request emits no batch"
    );
}
