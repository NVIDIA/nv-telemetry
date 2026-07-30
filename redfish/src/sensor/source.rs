// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use nv_redfish::core::EntityTypeRef;
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Instance;
use nv_telemetry_core::SourceKey;
use nv_telemetry_core::Subject;
use nv_telemetry_core::SubjectId;
use nv_telemetry_core::SubjectKind;

use crate::uri::canonical;
use crate::uri::sensor_scope_from_canonical;
use crate::uri::SensorScope;
use crate::FieldValue;

/// The subject kind every sensor observation is filed under.
const SENSOR: SubjectKind = SubjectKind::from_static("sensor");

/// The subject kind of a Chassis a sensor hangs from.
const CHASSIS: SubjectKind = SubjectKind::from_static("chassis");

/// The subject kind shared by every supported power equipment collection.
const POWER_DISTRIBUTION: SubjectKind = SubjectKind::from_static("power_distribution");

/// A non-empty, canonical source location for one Sensor.
#[derive(Debug)]
pub(super) struct SensorSource(SourceKey);

impl SensorSource {
    pub(super) fn from_sensor(sensor: &Sensor) -> FieldValue<Self> {
        let uri = sensor.odata_id().to_string();
        if uri.is_empty() {
            return FieldValue::invalid("@odata.id is empty");
        }
        FieldValue::present(Self(SourceKey::from(canonical(&uri).as_ref())))
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
        SensorSource::from_sensor(sensor).and_then(|source| {
            match sensor_scope_from_canonical(source.as_str()) {
                Some(scope) => FieldValue::present(Self { source, scope }),
                None => FieldValue::invalid(format!(
                    "URI is not in a Chassis or PowerDistribution Sensors collection: {}",
                    source.as_str()
                )),
            }
        })
    }

    pub(super) fn sensor_subject(&self, id: &SensorId) -> Subject {
        Subject::new(SENSOR, self.scoped_subject_id(id.as_str()))
    }

    /// Builds the subject of the collection the sensor hangs from.
    pub(super) fn parent_subject(&self) -> Subject {
        match &self.scope {
            SensorScope::Chassis(id) => Subject::new(CHASSIS, SubjectId::from(id.as_str())),
            SensorScope::PowerDistribution { collection, id } => Subject::new(
                POWER_DISTRIBUTION,
                SubjectId::from(format!("{collection}/{id}")),
            ),
        }
    }

    /// Qualifies a Sensor id with its parent scope.
    ///
    /// `Sensor.Id` is unique only within one Sensors collection, so the parent
    /// path is part of the identity rather than decoration on it.
    fn scoped_subject_id(&self, sensor_id: &str) -> SubjectId {
        match &self.scope {
            SensorScope::Chassis(id) => SubjectId::from(format!("chassis/{id}/{sensor_id}")),
            SensorScope::PowerDistribution { collection, id } => {
                SubjectId::from(format!("power_distribution/{collection}/{id}/{sensor_id}"))
            }
        }
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
