// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The static planner: needs in, planned polls out.
//!
//! Selection is deterministic and explainable — the first polled
//! declaration in list order serves every need — and validation is loud at
//! plan time: a declaration whose identity cannot form a wire `Origin`
//! fails here, never at status-build time. Capability probing, provider
//! preference, and demotion arrive with later milestones; a plan produced
//! here is complete because nothing in it can be unresolved yet.

use std::fmt;
use std::time::Duration;

use nv_telemetry_model::EndpointContext;
use nv_telemetry_model::Invalid;
use nv_telemetry_model::Origin;
use nv_telemetry_source::AcquisitionMode;
use nv_telemetry_source::ProviderDeclaration;

/// One thing the embedder wants polled: a protocol-scoped target on one
/// endpoint, at a cadence. The target is opaque to orchestration — for
/// Redfish it is the sensor's `OData` id.
#[derive(Clone, Debug)]
pub struct PollNeed {
    endpoint: EndpointContext,
    target: String,
    cadence: Duration,
}

impl PollNeed {
    /// A need for `target` on `endpoint`, polled every `cadence`.
    #[must_use]
    pub fn new(endpoint: EndpointContext, target: impl Into<String>, cadence: Duration) -> Self {
        Self {
            endpoint,
            target: target.into(),
            cadence,
        }
    }

    /// The endpoint the target lives on.
    #[must_use]
    pub fn endpoint(&self) -> &EndpointContext {
        &self.endpoint
    }

    /// The protocol-scoped locator of what to poll.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// How often to poll.
    #[must_use]
    pub fn cadence(&self) -> Duration {
        self.cadence
    }
}

/// One resolved poll: the need plus the provider the plan selected for it,
/// already spelled as the wire `Origin` every batch and status will carry.
#[derive(Clone, Debug)]
pub struct PlannedPoll {
    endpoint: EndpointContext,
    target: String,
    origin: Origin,
    cadence: Duration,
    cost: u64,
}

impl PlannedPoll {
    /// The endpoint the poll runs against.
    #[must_use]
    pub fn endpoint(&self) -> &EndpointContext {
        &self.endpoint
    }

    /// The protocol-scoped locator to poll.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The selected provider's identity.
    #[must_use]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// How often the poll runs.
    #[must_use]
    pub fn cadence(&self) -> Duration {
        self.cadence
    }

    /// The declared request weight, in dispatcher token units.
    #[must_use]
    pub fn cost(&self) -> u64 {
        self.cost
    }
}

/// The resolved plan: every need, served.
#[derive(Clone, Debug)]
pub struct Plan {
    polls: Vec<PlannedPoll>,
}

impl Plan {
    /// The planned polls, in need order.
    #[must_use]
    pub fn polls(&self) -> &[PlannedPoll] {
        &self.polls
    }
}

/// The longest cadence a plan accepts. Anything slower is a configuration
/// error: instant arithmetic near `Duration::MAX` would silently turn
/// "poll every N" into "poll once, then never again".
const MAX_CADENCE: Duration = Duration::from_hours(366 * 24);

/// Why a plan could not be produced.
#[derive(Debug)]
pub enum PlanError {
    /// No declaration offers polled acquisition.
    NoPolledProvider,
    /// More than one declaration offers polled acquisition.
    /// Need-to-provider matching does not exist yet, so a second provider
    /// must be a loud error rather than a silent list-order coin toss.
    AmbiguousProviders,
    /// A declaration's identity cannot form a wire `Origin`.
    InvalidDeclaration {
        /// The declared provider name, as far as it could be read.
        provider: String,
        /// What the origin rejected.
        error: Invalid,
    },
    /// A declaration's request cost is zero, which would disable rate
    /// limiting entirely.
    ZeroCost {
        /// The declared provider name.
        provider: String,
    },
    /// A need's cadence is zero or beyond the year-long maximum.
    InvalidCadence {
        /// The need's target.
        target: String,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPolledProvider => f.write_str("no declaration offers polled acquisition"),
            Self::AmbiguousProviders => f.write_str(
                "more than one declaration offers polled acquisition, and \
                 need-to-provider matching does not exist yet",
            ),
            Self::InvalidDeclaration { provider, error } => write!(
                f,
                "declaration for `{provider}` cannot form a wire origin: {error}"
            ),
            Self::ZeroCost { provider } => write!(
                f,
                "declaration for `{provider}` costs zero, which would disable \
                 rate limiting"
            ),
            Self::InvalidCadence { target } => write!(
                f,
                "cadence for `{target}` must be positive and at most a year"
            ),
        }
    }
}

impl std::error::Error for PlanError {}

/// Resolves needs against declarations: exactly one polled declaration
/// serves every need.
///
/// # Errors
///
/// [`PlanError::NoPolledProvider`] when nothing polls;
/// [`PlanError::AmbiguousProviders`] when more than one declaration does —
/// loud until need-to-provider matching exists;
/// [`PlanError::InvalidDeclaration`] and [`PlanError::ZeroCost`] when the
/// selected declaration cannot be honored; and
/// [`PlanError::InvalidCadence`] for a zero or beyond-a-year cadence.
pub fn plan(needs: Vec<PollNeed>, declarations: &[ProviderDeclaration]) -> Result<Plan, PlanError> {
    let mut polled = declarations
        .iter()
        .filter(|declaration| declaration.mode() == AcquisitionMode::Polled);
    let selected = polled.next().ok_or(PlanError::NoPolledProvider)?;
    if polled.next().is_some() {
        return Err(PlanError::AmbiguousProviders);
    }
    if selected.cost() == 0 {
        return Err(PlanError::ZeroCost {
            provider: selected.provider().to_owned(),
        });
    }
    let origin = Origin::builder()
        .provider(selected.provider())
        .request_class(selected.request_class())
        .build()
        .map_err(|error| PlanError::InvalidDeclaration {
            provider: selected.provider().to_owned(),
            error,
        })?;

    let polls = needs
        .into_iter()
        .map(|need| {
            if need.cadence.is_zero() || need.cadence > MAX_CADENCE {
                return Err(PlanError::InvalidCadence {
                    target: need.target,
                });
            }
            Ok(PlannedPoll {
                endpoint: need.endpoint,
                target: need.target,
                origin: origin.clone(),
                cadence: need.cadence,
                cost: selected.cost(),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Plan { polls })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> EndpointContext {
        EndpointContext::builder()
            .endpoint_id("bmc-lab-07")
            .build()
            .expect("a valid endpoint")
    }

    #[test]
    fn the_one_polled_declaration_serves_every_need() {
        let declarations = [ProviderDeclaration::polled(
            "redfish.sensor.odata",
            "sensor-read",
            1,
        )];
        let needs = vec![
            PollNeed::new(endpoint(), "/redfish/v1/S/1", Duration::from_secs(30)),
            PollNeed::new(endpoint(), "/redfish/v1/S/2", Duration::from_mins(1)),
        ];

        let plan = plan(needs, &declarations).expect("a polled declaration exists");
        assert_eq!(plan.polls().len(), 2);
        for poll in plan.polls() {
            assert_eq!(poll.origin().provider(), "redfish.sensor.odata");
            assert_eq!(poll.origin().request_class(), "sensor-read");
            assert_eq!(poll.cost(), 1);
        }
        assert_eq!(plan.polls()[0].target(), "/redfish/v1/S/1");
        assert_eq!(plan.polls()[1].cadence(), Duration::from_mins(1));
    }

    #[test]
    fn planning_fails_loudly_without_a_polled_provider() {
        let error = plan(Vec::new(), &[]).expect_err("nothing polls");
        assert!(matches!(error, PlanError::NoPolledProvider));
    }

    #[test]
    fn a_second_polled_provider_is_a_loud_error_not_a_coin_toss() {
        let declarations = [
            ProviderDeclaration::polled("redfish.sensor.odata", "sensor-read", 1),
            ProviderDeclaration::polled("redfish.sensor.telemetry", "report-read", 2),
        ];
        let error = plan(Vec::new(), &declarations).expect_err("matching does not exist yet");
        assert!(matches!(error, PlanError::AmbiguousProviders));
    }

    #[test]
    fn an_invalid_declaration_fails_at_plan_time() {
        let declarations = [ProviderDeclaration::polled("", "sensor-read", 1)];
        let error = plan(Vec::new(), &declarations).expect_err("an empty provider is invalid");
        assert!(
            matches!(&error, PlanError::InvalidDeclaration { provider, .. } if provider.is_empty())
        );

        let declarations = [ProviderDeclaration::polled("p", "c", 0)];
        let error = plan(Vec::new(), &declarations).expect_err("zero cost disables rating");
        assert!(matches!(error, PlanError::ZeroCost { .. }));
    }

    #[test]
    fn a_zero_or_unbounded_cadence_fails_at_plan_time() {
        let declarations = [ProviderDeclaration::polled("p", "c", 1)];
        for cadence in [Duration::ZERO, Duration::MAX] {
            let needs = vec![PollNeed::new(endpoint(), "/redfish/v1/S/1", cadence)];
            let error = plan(needs, &declarations).expect_err("an unusable cadence is refused");
            assert!(matches!(error, PlanError::InvalidCadence { .. }));
        }
    }
}
