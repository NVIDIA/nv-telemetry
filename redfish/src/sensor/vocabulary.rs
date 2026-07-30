// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fmt;

use nv_redfish::schema::resource::Health as RedfishHealth;
use nv_redfish::schema::resource::State as RedfishState;
use nv_redfish::schema::sensor::ReadingType;
use nv_redfish::schema::sensor::ThresholdActivation;
use nv_telemetry_core::Health;
use nv_telemetry_core::Metric;
use nv_telemetry_core::OperatingState;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ReadingKind;

use crate::FieldValue;

/// Fallibly projects a source vocabulary into a typed local model.
///
/// This local trait permits conversions between Redfish and telemetry types
/// that cannot implement the standard library's `TryFrom` due to the orphan
/// rule.
pub(super) trait TryProjectInto<T> {
    type Error;

    fn try_project_into(self) -> Result<T, Self::Error>;
}

pub(super) fn project_optional<Source, Target>(value: Option<Source>) -> FieldValue<Target>
where
    Source: TryProjectInto<Target>,
    Source::Error: fmt::Display,
{
    value.map_or_else(FieldValue::missing, |value| {
        FieldValue::from_result(value.try_project_into())
    })
}

impl TryProjectInto<Health> for RedfishHealth {
    type Error = &'static str;

    fn try_project_into(self) -> Result<Health, Self::Error> {
        let value = match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Critical => "critical",
            Self::UnsupportedValue => return Err("unsupported or OEM Status.Health"),
        };
        Ok(Health::from_static(value))
    }
}

impl TryProjectInto<OperatingState> for RedfishState {
    type Error = &'static str;

    fn try_project_into(self) -> Result<OperatingState, Self::Error> {
        let value = match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::StandbyOffline => "standby_offline",
            Self::StandbySpare => "standby_spare",
            Self::InTest => "in_test",
            Self::Starting => "starting",
            Self::Absent => "absent",
            Self::UnavailableOffline => "unavailable_offline",
            Self::Deferring => "deferring",
            Self::Quiesced => "quiesced",
            Self::Updating => "updating",
            Self::Qualified => "qualified",
            Self::Degraded => "degraded",
            Self::UnsupportedValue => return Err("unsupported or OEM Status.State"),
        };
        Ok(OperatingState::from_static(value))
    }
}

/// The metric identity and accumulation semantics implied by `ReadingType`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReadingSemantics {
    metric: Metric,
    kind: ReadingKind,
}

impl ReadingSemantics {
    pub(super) fn into_parts(self) -> (Metric, ReadingKind) {
        (self.metric, self.kind)
    }
}

impl TryProjectInto<ReadingSemantics> for ReadingType {
    type Error = &'static str;

    fn try_project_into(self) -> Result<ReadingSemantics, Self::Error> {
        let (metric, kind) = match self {
            Self::Temperature => ("temperature", ReadingKind::Gauge),
            Self::Humidity => ("humidity", ReadingKind::Gauge),
            Self::Power => ("power", ReadingKind::Gauge),
            Self::EnergykWh => ("energy_kwh", ReadingKind::Counter),
            Self::EnergyJoules => ("energy_joules", ReadingKind::Counter),
            Self::EnergyWh => ("energy_wh", ReadingKind::Counter),
            Self::ChargeAh => ("charge_ah", ReadingKind::Counter),
            Self::Voltage => ("voltage", ReadingKind::Gauge),
            Self::Current => ("current", ReadingKind::Gauge),
            Self::Frequency => ("frequency", ReadingKind::Gauge),
            Self::Pressure => ("pressure", ReadingKind::Gauge),
            Self::PressurekPa => ("pressure_kpa", ReadingKind::Gauge),
            Self::PressurePa => ("pressure_pa", ReadingKind::Gauge),
            Self::LiquidLevel => ("liquid_level", ReadingKind::Gauge),
            Self::Rotational => ("rotational", ReadingKind::Gauge),
            Self::AirFlow => ("air_flow", ReadingKind::Gauge),
            Self::AirFlowCmm => ("air_flow_cmm", ReadingKind::Gauge),
            Self::LiquidFlow => ("liquid_flow", ReadingKind::Gauge),
            Self::LiquidFlowLpm => ("liquid_flow_lpm", ReadingKind::Gauge),
            Self::Barometric => ("barometric", ReadingKind::Gauge),
            Self::Altitude => ("altitude", ReadingKind::Gauge),
            Self::Percent => ("percent", ReadingKind::Gauge),
            Self::AbsoluteHumidity => ("absolute_humidity", ReadingKind::Gauge),
            Self::Heat => ("heat", ReadingKind::Gauge),
            Self::LinearPosition => ("linear_position", ReadingKind::Gauge),
            Self::LinearVelocity => ("linear_velocity", ReadingKind::Gauge),
            Self::LinearAcceleration => ("linear_acceleration", ReadingKind::Gauge),
            Self::RotationalPosition => ("rotational_position", ReadingKind::Gauge),
            Self::RotationalVelocity => ("rotational_velocity", ReadingKind::Gauge),
            Self::RotationalAcceleration => ("rotational_acceleration", ReadingKind::Gauge),
            Self::Valve => ("valve", ReadingKind::Gauge),
            Self::UnsupportedValue => return Err("unsupported or OEM ReadingType"),
        };
        Ok(ReadingSemantics {
            metric: Metric::from_static(metric),
            kind,
        })
    }
}

/// Model vocabulary for a Redfish threshold's trigger direction.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ProjectedThresholdActivation(&'static str);

impl ProjectedThresholdActivation {
    pub(super) fn into_property_value(self) -> PropertyValue {
        PropertyValue::String(self.0.into())
    }
}

impl TryProjectInto<ProjectedThresholdActivation> for ThresholdActivation {
    type Error = &'static str;

    fn try_project_into(self) -> Result<ProjectedThresholdActivation, Self::Error> {
        let value = match self {
            Self::Increasing => "increasing",
            Self::Decreasing => "decreasing",
            Self::Either => "either",
            Self::Disabled => "disabled",
            Self::UnsupportedValue => return Err("unsupported or OEM Activation"),
        };
        Ok(ProjectedThresholdActivation(value))
    }
}

#[cfg(test)]
mod tests {
    use nv_redfish::schema::resource::Health as RedfishHealth;
    use nv_redfish::schema::resource::State as RedfishState;
    use nv_redfish::schema::sensor::ReadingType;
    use nv_redfish::schema::sensor::ThresholdActivation;
    use nv_telemetry_core::Health;
    use nv_telemetry_core::OperatingState;
    use nv_telemetry_core::PropertyValue;
    use nv_telemetry_core::ReadingKind;

    use super::ProjectedThresholdActivation;
    use super::ReadingSemantics;
    use super::TryProjectInto;

    #[test]
    fn conversion_trait_projects_foreign_status_vocabularies() {
        let health: Health = RedfishHealth::Warning.try_project_into().unwrap();
        let state: OperatingState = RedfishState::StandbyOffline.try_project_into().unwrap();

        assert_eq!(health.as_str(), "warning");
        assert_eq!(state.as_str(), "standby_offline");
        assert_eq!(
            RedfishHealth::UnsupportedValue.try_project_into(),
            Err::<Health, _>("unsupported or OEM Status.Health")
        );
        assert_eq!(
            RedfishState::UnsupportedValue.try_project_into(),
            Err::<OperatingState, _>("unsupported or OEM Status.State")
        );
    }

    #[test]
    fn conversion_trait_projects_reading_semantics() {
        let semantics: ReadingSemantics = ReadingType::EnergykWh.try_project_into().unwrap();
        let (metric, kind) = semantics.into_parts();

        assert_eq!(metric.as_str(), "energy_kwh");
        assert_eq!(kind, ReadingKind::Counter);
        assert_eq!(
            ReadingType::UnsupportedValue.try_project_into(),
            Err::<ReadingSemantics, _>("unsupported or OEM ReadingType")
        );
    }

    #[test]
    fn conversion_trait_projects_threshold_vocabulary() {
        let activation: ProjectedThresholdActivation =
            ThresholdActivation::Increasing.try_project_into().unwrap();

        assert_eq!(
            activation.into_property_value(),
            PropertyValue::String("increasing".into())
        );
        assert_eq!(
            ThresholdActivation::UnsupportedValue.try_project_into(),
            Err::<ProjectedThresholdActivation, _>("unsupported or OEM Activation")
        );
    }
}
