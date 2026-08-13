// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The single-sensor read: one `OData` GET, up to two batches.

use std::fmt;
use std::sync::Arc;

use nv_redfish::core::ODataId;
use nv_redfish::schema::sensor::Sensor;
use nv_redfish::Bmc;
use nv_telemetry_model::Completeness;
use nv_telemetry_model::Coverage;
use nv_telemetry_model::EndpointContext;
use nv_telemetry_model::Invalid;
use nv_telemetry_model::Origin;
use nv_telemetry_model::Payload;
use nv_telemetry_model::Readings;
use nv_telemetry_model::States;
use nv_telemetry_source::Acquire;
use nv_telemetry_source::AcquisitionFailure;
use nv_telemetry_source::AcquisitionFailureClass;
use nv_telemetry_source::AcquisitionParts;

use crate::failure::ClassifyError;
use crate::projection::project_sensor;
use crate::projection::SensorParts;

/// One sensor, read over one endpoint's `Bmc`.
///
/// A dispatched leaf: the planner names the sensor URI, the dispatcher
/// decides when this runs, and this type only knows how — GET the document,
/// project it, assemble the batches. Generic over the transport so the same
/// provider runs against HTTP and against the mock the corpus replays
/// through.
pub struct SensorRead<B> {
    endpoint: EndpointContext,
    origin: Origin,
    sensor: ODataId,
    /// The requested location string. Generated subject matchers canonicalize
    /// it before deriving identity, which never comes from the payload's own
    /// claim.
    location: String,
    bmc: Arc<B>,
}

impl<B> fmt::Debug for SensorRead<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A requested URI may carry query credentials, and a transport may
        // own authentication material. Scheduling identity is sufficient to
        // identify this task without exposing either one.
        f.debug_struct("SensorRead")
            .field("endpoint_id", &self.endpoint.endpoint_id())
            .field("provider", &self.origin.provider())
            .field("request_class", &self.origin.request_class())
            .field("sensor", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<B> Clone for SensorRead<B> {
    fn clone(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            origin: self.origin.clone(),
            sensor: self.sensor.clone(),
            location: self.location.clone(),
            bmc: Arc::clone(&self.bmc),
        }
    }
}

impl<B> SensorRead<B> {
    /// Provider identity, as `Origin.provider` carries it.
    pub const PROVIDER: &'static str = "redfish.sensor.odata";

    /// Request class, as dispatcher lanes and breakers key it.
    pub const REQUEST_CLASS: &'static str = "sensor-read";

    /// A read of `sensor` on the endpoint `bmc` reaches.
    ///
    /// # Panics
    ///
    /// Never in practice: the origin is built from this provider's own
    /// constants, and the unit test below pins that they satisfy the
    /// origin's bounds.
    #[must_use]
    pub fn new(endpoint: EndpointContext, sensor: ODataId, bmc: Arc<B>) -> Self {
        let origin = Origin::builder()
            .provider(Self::PROVIDER)
            .request_class(Self::REQUEST_CLASS)
            .build()
            .expect("the provider's own constants satisfy the origin's bounds");
        let location = sensor.to_string();
        Self {
            endpoint,
            origin,
            sensor,
            location,
            bmc,
        }
    }

    /// Assembles the batches the projection's parts call for: a readings
    /// batch when a descriptor exists (zero or one sample — a descriptor
    /// with no sample is the null-reading story, and the sample-key rule
    /// holds trivially), a states batch when there are observations, and no
    /// batch at all otherwise.
    fn assemble(parts: SensorParts) -> Result<AcquisitionParts, Invalid> {
        let mut payloads = Vec::new();
        let coverage = Coverage::builder()
            .completeness(Completeness::Partial)
            .build()?;
        // Samples without descriptors cannot be silently dropped: either the
        // payload builder accepts them or its refusal surfaces as the
        // residual tier — never a reading that vanishes.
        if !parts.signal_descriptors.is_empty() || !parts.readings.is_empty() {
            let readings = Readings::builder()
                .descriptors(parts.signal_descriptors)
                .samples(parts.readings)
                .build()?;
            payloads.push((coverage.clone(), Payload::Readings(readings)));
        }
        if !parts.state_observations.is_empty() {
            let states = States::builder()
                .observations(parts.state_observations)
                .build()?;
            payloads.push((coverage, Payload::States(states)));
        }
        Ok(AcquisitionParts::new(payloads, parts.issues))
    }
}

/// A projection or assembly failure past the triage tiers is this crate's
/// bug: an operational fact for the status stream, never device data.
fn internal_bug(error: &Invalid) -> AcquisitionFailure {
    AcquisitionFailure::new(AcquisitionFailureClass::Internal)
        .with_retryable(false)
        .with_detail(format!("projection bug: {error}"))
}

impl<B> Acquire for SensorRead<B>
where
    B: Bmc,
    B::Error: ClassifyError,
{
    type Output = AcquisitionParts;

    fn endpoint(&self) -> &EndpointContext {
        &self.endpoint
    }

    fn origin(&self) -> &Origin {
        &self.origin
    }

    async fn perform(&self) -> Result<AcquisitionParts, AcquisitionFailure> {
        let sensor = self
            .bmc
            .get::<Sensor>(&self.sensor)
            .await
            .map_err(|error| error.classify())?;
        let parts =
            project_sensor(&sensor, &self.location).map_err(|error| internal_bug(&error))?;
        Self::assemble(parts).map_err(|error| internal_bug(&error))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::sync::Arc;

    use nv_telemetry_model::EndpointContext;
    use nv_telemetry_model::Origin;
    use nv_telemetry_model::StateObservation;
    use nv_telemetry_source::AcquisitionFailureClass;

    use super::internal_bug;
    use super::SensorRead;

    struct NonCloneBmc;

    struct SensitiveBmc;

    impl fmt::Debug for SensitiveBmc {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("transport-secret")
        }
    }

    fn assert_clone<T: Clone>() {}

    #[test]
    fn sharing_a_read_does_not_require_the_transport_to_be_clone() {
        assert_clone::<SensorRead<NonCloneBmc>>();
    }

    #[test]
    fn debug_exposes_only_scheduling_identity() {
        let read = SensorRead::new(
            EndpointContext::builder()
                .endpoint_id("endpoint-a")
                .build()
                .expect("a valid endpoint"),
            "/redfish/v1/Chassis/1/Sensors/CPU?token=query-secret"
                .to_owned()
                .into(),
            Arc::new(SensitiveBmc),
        );

        let rendered = format!("{read:?}");
        assert!(rendered.contains("endpoint-a"));
        assert!(rendered.contains(SensorRead::<SensitiveBmc>::PROVIDER));
        assert!(!rendered.contains("/redfish/"));
        assert!(!rendered.contains("query-secret"));
        assert!(!rendered.contains("transport-secret"));
    }

    #[test]
    fn the_providers_constants_satisfy_the_origins_bounds() {
        // `SensorRead::new` builds its origin with `expect` on this exact
        // invariant; this is the pin that keeps that expect unreachable.
        let origin = Origin::builder()
            .provider(SensorRead::<()>::PROVIDER)
            .request_class(SensorRead::<()>::REQUEST_CLASS)
            .build()
            .expect("provider constants are valid origin fields");
        assert_eq!(origin.provider(), "redfish.sensor.odata");
        assert_eq!(origin.request_class(), "sensor-read");
    }

    #[test]
    fn a_synthetic_plan_model_disagreement_reaches_the_internal_tripwire() {
        // Compilation proves every supported projection plan covers required
        // fields. Bypass that boundary deliberately to pin the one residual
        // tier: if generated assembly and the model ever disagree, the
        // refusal is operational Internal, never a device projection issue.
        let mismatch = StateObservation::builder()
            .build()
            .expect_err("an observation without its planned fields is invalid");
        let failure = internal_bug(&mismatch);

        assert_eq!(failure.class(), AcquisitionFailureClass::Internal);
        assert_eq!(failure.retryable(), Some(false));
        assert!(
            failure
                .detail()
                .is_some_and(|detail| detail.starts_with("projection bug: ")),
            "the mismatch remains operator-visible: {failure:?}"
        );
    }
}
