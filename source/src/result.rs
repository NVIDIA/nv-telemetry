// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! What one acquisition yields: batches and issues, or a classified failure.
//!
//! The outcome is `Result<Acquired, AcquisitionFailure>`, and the split is
//! the contract's central invariant made unrepresentable: a failed request
//! emits no batch at all — never an empty one, never one with zeroes. A
//! response that arrived with some fields unusable is the `Ok` side carrying
//! issues; partial *failure* needs no representation because one admitted
//! unit is one request, and per-resource units fail separately.

use std::fmt;

use nv_telemetry_model::Coverage;
use nv_telemetry_model::EndpointContext;
use nv_telemetry_model::Invalid;
use nv_telemetry_model::ObservationBatch;
use nv_telemetry_model::ObservationWindow;
use nv_telemetry_model::Origin;
use nv_telemetry_model::Payload;
use nv_telemetry_model::Timestamp;

use crate::ProjectionIssue;

pub(crate) const DETAIL_TRUNCATION_MARKER: &str = "...";

type CoveredPayloads = Box<[(Coverage, Payload)]>;

/// A failure classification a source can originate.
///
/// Deliberately closed over the classes this build understands. The model's
/// [`FailureClass`](nv_telemetry_model::FailureClass) also has an
/// `Unrecognized(i32)` arm so an older *consumer* can preserve a future wire
/// value; admitting that arm here would let a producer emit an unspecified or
/// otherwise unknown classification that its own policy cannot interpret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AcquisitionFailureClass {
    /// The endpoint could not be reached.
    Connectivity,
    /// The endpoint refused the credentials.
    Authentication,
    /// The request ran out of time.
    Timeout,
    /// The endpoint answered with something the provider could not parse.
    Protocol,
    /// The endpoint does not support what the provider asked of it.
    Unsupported,
    /// The endpoint reported an internal error of its own.
    Device,
    /// The failure was ours.
    Internal,
}

impl From<AcquisitionFailureClass> for nv_telemetry_model::FailureClass {
    fn from(value: AcquisitionFailureClass) -> Self {
        match value {
            AcquisitionFailureClass::Connectivity => Self::Connectivity,
            AcquisitionFailureClass::Authentication => Self::Authentication,
            AcquisitionFailureClass::Timeout => Self::Timeout,
            AcquisitionFailureClass::Protocol => Self::Protocol,
            AcquisitionFailureClass::Unsupported => Self::Unsupported,
            AcquisitionFailureClass::Device => Self::Device,
            AcquisitionFailureClass::Internal => Self::Internal,
        }
    }
}

/// Envelope-free output from one successful collection provider.
///
/// Keeping identity and time out of this provider-facing type is deliberate:
/// [`crate::acquire`] stamps them from the admitted [`crate::Acquire`] unit
/// and its caller-supplied instant after the provider hook returns.
#[derive(Clone, Debug, PartialEq)]
pub struct AcquisitionParts {
    payloads: CoveredPayloads,
    issues: Box<[ProjectionIssue]>,
}

impl AcquisitionParts {
    /// Collects validated payloads and the source fields that produced none.
    #[must_use]
    pub fn new(payloads: Vec<(Coverage, Payload)>, issues: Vec<ProjectionIssue>) -> Self {
        Self {
            payloads: payloads.into_boxed_slice(),
            issues: issues.into_boxed_slice(),
        }
    }

    /// The homogeneous payloads and their coverage declarations.
    #[must_use]
    pub fn payloads(&self) -> &[(Coverage, Payload)] {
        &self.payloads
    }

    /// The source fields that projected to nothing, and why.
    #[must_use]
    pub fn issues(&self) -> &[ProjectionIssue] {
        &self.issues
    }

    pub(crate) fn into_parts(self) -> (CoveredPayloads, Box<[ProjectionIssue]>) {
        (self.payloads, self.issues)
    }
}

/// Everything one completed acquisition produced: zero or more homogeneous
/// batches, plus the issues for source fields that projected to nothing.
///
/// Empty batches with issues is the honest result of a response that
/// answered and was wholly unusable — the device spoke, so it is not an
/// acquisition failure, and no batch is better than a fabricated one.
#[derive(Clone, Debug, PartialEq)]
pub struct Acquired {
    batches: Box<[ObservationBatch]>,
    issues: Box<[ProjectionIssue]>,
}

impl Acquired {
    pub(crate) fn from_parts(
        endpoint: &EndpointContext,
        origin: &Origin,
        at: Timestamp,
        parts: AcquisitionParts,
    ) -> Result<Self, Invalid> {
        let (payloads, issues) = parts.into_parts();
        let window = ObservationWindow::builder().start(at).build()?;
        let batches = payloads
            .into_vec()
            .into_iter()
            .map(|(coverage, payload)| {
                ObservationBatch::builder()
                    .endpoint(endpoint.clone())
                    .origin(origin.clone())
                    .window(window.clone())
                    .coverage(coverage)
                    .payload(payload)
                    .build()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            batches: batches.into_boxed_slice(),
            issues,
        })
    }

    /// The homogeneous batches this acquisition produced.
    #[must_use]
    pub fn batches(&self) -> &[ObservationBatch] {
        &self.batches
    }

    /// The source fields that projected to nothing, and why.
    #[must_use]
    pub fn issues(&self) -> &[ProjectionIssue] {
        &self.issues
    }

    /// Consumes into batches and issues.
    #[must_use]
    pub fn into_parts(self) -> (Box<[ObservationBatch]>, Box<[ProjectionIssue]>) {
        (self.batches, self.issues)
    }
}

/// A classified acquisition failure: exactly the facts dispatcher and
/// planner policy run on, and nothing a caller could branch on protocol
/// with.
///
/// Deliberately no source-error field. Orchestration must never downcast to
/// a protocol error type, and omitting the field enforces the boundary this
/// crate exists for; what matters folds into [`detail`](Self::detail), which
/// is precisely what reaches the status stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcquisitionFailure {
    class: AcquisitionFailureClass,
    retryable: Option<bool>,
    detail: Option<String>,
}

impl AcquisitionFailure {
    /// A failure of the given class, with no refinement.
    #[must_use]
    pub fn new(class: AcquisitionFailureClass) -> Self {
        Self {
            class,
            retryable: None,
            detail: None,
        }
    }

    /// Refines retryability beyond the class's policy default — a 401 and a
    /// 403 classify alike but retry differently.
    #[must_use]
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    /// Adds an operator-facing cause. Callers must pass derived, non-secret
    /// text rather than raw transport displays or response bodies. An empty
    /// string is stored as no detail, because the status wire type rejects
    /// present-but-empty detail. Overlong text is UTF-8-safely truncated with
    /// a marker so every failure can be copied into `AcquisitionStatus`
    /// without introducing a second validation failure.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let limit = nv_telemetry_model::limits::ACQUISITIONSTATUS_DETAIL_MAX_LEN as usize;
        self.detail = bounded(detail.into(), limit);
        self
    }

    /// The classification dispatcher and planner policy key off.
    #[must_use]
    pub fn class(&self) -> AcquisitionFailureClass {
        self.class
    }

    /// Whether retrying could plausibly succeed; `None` means the class's
    /// policy default applies.
    #[must_use]
    pub fn retryable(&self) -> Option<bool> {
        self.retryable
    }

    /// Operator-facing cause, when one was captured.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// Empty text becomes `None`; overlong text is UTF-8-safely truncated to
/// `limit` with a marker. Both callers bound `detail` fields, whose
/// contracts reject present-but-empty, so `None` is the only honest
/// spelling of nothing there; a field where empty is legal data must not
/// come through here.
pub(crate) fn bounded(mut text: String, limit: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    if text.len() <= limit {
        return Some(text);
    }

    if limit < DETAIL_TRUNCATION_MARKER.len() {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        return (!text.is_empty()).then_some(text);
    }

    let mut end = limit - DETAIL_TRUNCATION_MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(DETAIL_TRUNCATION_MARKER);
    Some(text)
}

impl fmt::Display for AcquisitionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "acquisition failed: {:?}", self.class)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AcquisitionFailure {}

#[cfg(test)]
mod tests {
    use nv_telemetry_model::Completeness;
    use nv_telemetry_model::EndpointContext;
    use nv_telemetry_model::Origin;
    use nv_telemetry_model::States;

    use super::*;
    use crate::acquire;
    use crate::Acquire;

    struct FixtureAcquisition {
        endpoint: EndpointContext,
        origin: Origin,
    }

    impl Acquire for FixtureAcquisition {
        type Output = AcquisitionParts;

        fn endpoint(&self) -> &EndpointContext {
            &self.endpoint
        }

        fn origin(&self) -> &Origin {
            &self.origin
        }

        async fn perform(&self) -> Result<Self::Output, AcquisitionFailure> {
            let coverage = Coverage::builder()
                .completeness(Completeness::Partial)
                .build()
                .expect("valid coverage");
            let payload = || {
                Payload::States(
                    States::builder()
                        .build()
                        .expect("an empty states payload is valid"),
                )
            };
            Ok(AcquisitionParts::new(
                vec![(coverage.clone(), payload()), (coverage, payload())],
                Vec::new(),
            ))
        }
    }

    #[tokio::test]
    async fn batch_envelopes_come_from_the_acquisition_identity_and_callers_instant() {
        let acquisition = FixtureAcquisition {
            endpoint: EndpointContext::builder()
                .endpoint_id("endpoint-a")
                .build()
                .expect("a valid endpoint"),
            origin: Origin::builder()
                .provider("provider-a")
                .request_class("request-a")
                .build()
                .expect("a valid origin"),
        };
        let at = Timestamp::new(123, 456).expect("a valid instant");

        let acquired = acquire(&acquisition, at)
            .await
            .expect("validated parts form batch envelopes");

        assert_eq!(acquired.batches().len(), 2);
        for batch in acquired.batches() {
            assert_eq!(batch.endpoint(), acquisition.endpoint());
            assert_eq!(batch.origin(), acquisition.origin());
            assert_eq!(batch.window().start(), &at);
            assert!(batch.window().end().is_none());
        }
    }

    #[test]
    fn an_empty_detail_is_no_detail() {
        // The status wire type rejects present-but-empty detail, so the
        // failure must not smuggle one toward it.
        let failure = AcquisitionFailure::new(AcquisitionFailureClass::Timeout).with_detail("");
        assert_eq!(failure.detail(), None);

        let failure =
            AcquisitionFailure::new(AcquisitionFailureClass::Timeout).with_detail("30s elapsed");
        assert_eq!(failure.detail(), Some("30s elapsed"));
    }

    #[test]
    fn detail_never_exceeds_the_status_contract() {
        let limit = nv_telemetry_model::limits::ACQUISITIONSTATUS_DETAIL_MAX_LEN as usize;
        let exact = "x".repeat(limit);
        let failure =
            AcquisitionFailure::new(AcquisitionFailureClass::Internal).with_detail(exact.clone());
        assert_eq!(failure.detail(), Some(exact.as_str()));

        let failure = AcquisitionFailure::new(AcquisitionFailureClass::Internal)
            .with_detail("x".repeat(limit + 1));
        let detail = failure
            .detail()
            .expect("overlong detail is retained safely");
        assert_eq!(detail.len(), limit);
        assert!(detail.ends_with(DETAIL_TRUNCATION_MARKER));

        // Put the nominal cut point inside a four-byte scalar. Truncation must
        // move back to a character boundary before appending the marker.
        let unicode = format!("{}💥z", "x".repeat(limit - 4));
        let failure =
            AcquisitionFailure::new(AcquisitionFailureClass::Internal).with_detail(unicode);
        let detail = failure.detail().expect("unicode detail remains present");
        assert!(detail.len() <= limit);
        assert!(detail.ends_with(DETAIL_TRUNCATION_MARKER));

        let status = nv_telemetry_model::AcquisitionStatus::builder()
            .endpoint_id("endpoint")
            .provider("provider")
            .request_class("request")
            .outcome(nv_telemetry_model::Outcome::Failed)
            .failure_class(failure.class().into())
            .started_at(Timestamp::new(0, 0).expect("the epoch is valid"))
            .detail(detail)
            .build()
            .expect("bounded failure detail copies into acquisition status");
        assert_eq!(status.detail(), Some(detail));
    }

    #[test]
    fn producer_classes_convert_only_to_known_model_values() {
        use nv_telemetry_model::FailureClass as ModelFailureClass;

        let rows = [
            (
                AcquisitionFailureClass::Connectivity,
                ModelFailureClass::Connectivity,
            ),
            (
                AcquisitionFailureClass::Authentication,
                ModelFailureClass::Authentication,
            ),
            (AcquisitionFailureClass::Timeout, ModelFailureClass::Timeout),
            (
                AcquisitionFailureClass::Protocol,
                ModelFailureClass::Protocol,
            ),
            (
                AcquisitionFailureClass::Unsupported,
                ModelFailureClass::Unsupported,
            ),
            (AcquisitionFailureClass::Device, ModelFailureClass::Device),
            (
                AcquisitionFailureClass::Internal,
                ModelFailureClass::Internal,
            ),
        ];
        for (source, model) in rows {
            assert_eq!(ModelFailureClass::from(source), model);
        }
    }
}
