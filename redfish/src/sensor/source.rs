// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use nv_redfish::core::EntityTypeRef;
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Instance;
use nv_telemetry_core::SourceKey;
use nv_telemetry_core::Subject;
use nv_telemetry_core::SubjectKind;

use crate::uri::canonical_owned;
use crate::uri::sensor_scope_from_canonical;
use crate::uri::SensorScope;
use crate::FieldValue;

/// The subject kind every sensor observation is filed under.
const SENSOR: SubjectKind = SubjectKind::from_static("sensor");

/// A non-empty, canonical source location for one Sensor.
#[derive(Debug)]
pub(super) struct SensorSource(SourceKey);

impl SensorSource {
    pub(super) fn from_sensor(sensor: &Sensor) -> FieldValue<Self> {
        let uri = sensor.odata_id().to_string();
        if uri.is_empty() {
            return FieldValue::invalid("@odata.id is empty");
        }
        FieldValue::present(Self(SourceKey::from(canonical_owned(uri))))
    }

    pub(super) fn into_source_key(self) -> SourceKey {
        self.0
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A validated source whose URI also identifies a supported Sensor scope.
#[derive(Debug)]
pub(super) struct SensorLocation {
    source: SensorSource,
    scope: SensorScope,
}

impl SensorLocation {
    pub(super) fn from_sensor(sensor: &Sensor) -> FieldValue<Self> {
        let source = match SensorSource::from_sensor(sensor) {
            FieldValue::Present(source) => source,
            FieldValue::Missing => return FieldValue::missing(),
            FieldValue::Invalid(detail) => return FieldValue::invalid(detail),
        };
        match sensor_scope_from_canonical(source.as_str()) {
            Some(scope) => FieldValue::present(Self { source, scope }),
            None => FieldValue::invalid(format!(
                "URI is not in a Chassis or PowerDistribution Sensors collection: {}",
                source.as_str()
            )),
        }
    }

    pub(super) fn sensor_subject(&self, id: &SensorId) -> Subject {
        Subject::new(SENSOR, self.scope.sensor_id(id.as_str()))
    }

    pub(super) fn parent_subject(&self) -> Subject {
        self.scope.parent_subject()
    }

    pub(super) fn into_source_key(self) -> SourceKey {
        self.source.into_source_key()
    }
}

/// A non-empty Sensor identifier suitable for scoped subject construction.
#[derive(Debug)]
pub(super) struct SensorId(Instance);

impl SensorId {
    pub(super) fn from_sensor(sensor: &Sensor) -> FieldValue<Self> {
        if sensor.base.id.is_empty() {
            FieldValue::invalid("Sensor.Id is empty")
        } else {
            FieldValue::present(Self(Instance::from(sensor.base.id.as_str())))
        }
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(super) fn into_instance(self) -> Instance {
        self.0
    }
}
