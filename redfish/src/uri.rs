// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::borrow::Cow;

use nv_telemetry_core::Subject;
use nv_telemetry_core::SubjectId;
use nv_telemetry_core::SubjectKind;

const CHASSIS: SubjectKind = SubjectKind::from_static("chassis");
const POWER_DISTRIBUTION: SubjectKind = SubjectKind::from_static("power_distribution");

/// The parent collection scope encoded by a Sensor URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SensorScope {
    Chassis(String),
    PowerDistribution {
        collection: &'static str,
        id: String,
    },
}

impl SensorScope {
    pub(crate) fn parent_subject(&self) -> Subject {
        match self {
            Self::Chassis(id) => Subject::new(CHASSIS, SubjectId::from(id.clone())),
            Self::PowerDistribution { collection, id } => Subject::new(
                POWER_DISTRIBUTION,
                SubjectId::from(format!("{collection}/{id}")),
            ),
        }
    }

    pub(crate) fn sensor_id(&self, sensor_id: &str) -> SubjectId {
        match self {
            Self::Chassis(id) => SubjectId::from(format!("chassis/{id}/{sensor_id}")),
            Self::PowerDistribution { collection, id } => {
                SubjectId::from(format!("power_distribution/{collection}/{id}/{sensor_id}"))
            }
        }
    }
}

/// Reduces an absolute or relative Redfish URI to its endpoint-local path.
///
/// Scheme and authority, query, fragment, and trailing separators do not
/// distinguish a Redfish resource. Relative paths are rooted so all
/// acquisition routes produce the same key.
pub(crate) fn canonical(uri: &str) -> Cow<'_, str> {
    let path = if let Some(scheme) = uri.find("://") {
        let authority = &uri[scheme + 3..];
        path_after_authority(authority)
    } else if let Some(authority) = uri.strip_prefix("//") {
        path_after_authority(authority)
    } else {
        uri
    };
    let end = path
        .char_indices()
        .find_map(|(index, character)| matches!(character, '?' | '#').then_some(index))
        .unwrap_or(path.len());
    let path = path[..end].trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    if path.starts_with('/') {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(format!("/{path}"))
    }
}

/// Canonicalizes an owned URI without replacing an already-canonical buffer.
pub(crate) fn canonical_owned(uri: String) -> String {
    let replacement = {
        let canonical = canonical(&uri);
        (canonical.as_ref() != uri).then(|| canonical.into_owned())
    };
    replacement.unwrap_or(uri)
}

fn path_after_authority(authority: &str) -> &str {
    authority
        .char_indices()
        .find_map(|(index, character)| {
            matches!(character, '/' | '?' | '#').then_some((index, character))
        })
        .and_then(|(index, character)| (character == '/').then_some(&authority[index..]))
        .unwrap_or("/")
}

/// Returns the supported parent collection scope encoded by a Sensor URI.
#[cfg(test)]
pub(crate) fn sensor_scope(uri: &str) -> Option<SensorScope> {
    let canonical = canonical(uri);
    sensor_scope_from_canonical(&canonical)
}

pub(crate) fn sensor_scope_from_canonical(canonical: &str) -> Option<SensorScope> {
    let mut segments = canonical.split('/').filter(|segment| !segment.is_empty());
    let first = segments.next()?;
    let resource = if first == "redfish" {
        if segments.next()? != "v1" {
            return None;
        }
        segments.next()?
    } else {
        first
    };

    match resource {
        "Chassis" => {
            let parent = segments.next()?;
            if segments.next()? != "Sensors"
                || segments.next().is_none()
                || segments.next().is_some()
            {
                return None;
            }
            Some(SensorScope::Chassis(parent.to_owned()))
        }
        "PowerEquipment" => {
            let collection = power_distribution_collection(segments.next()?)?;
            let parent = segments.next()?;
            if segments.next()? != "Sensors"
                || segments.next().is_none()
                || segments.next().is_some()
            {
                return None;
            }
            Some(SensorScope::PowerDistribution {
                collection,
                id: parent.to_owned(),
            })
        }
        _ => None,
    }
}

fn power_distribution_collection(collection: &str) -> Option<&'static str> {
    match collection {
        "FloorPDUs" => Some("floor_pdus"),
        "RackPDUs" => Some("rack_pdus"),
        "Switchgear" => Some("switchgear"),
        "TransferSwitches" => Some("transfer_switches"),
        "PowerShelves" => Some("power_shelves"),
        "ElectricalBuses" => Some("electrical_buses"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::canonical;
    use super::sensor_scope;
    use super::SensorScope;

    #[test]
    fn uri_variants_name_the_same_resource() {
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp/"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("https://bmc.example/redfish/v1/Chassis/1/Sensors/CPU0Temp/?x=1#Reading"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("redfish/v1/Chassis/1/Sensors/CPU0Temp?x=1"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
    }

    #[test]
    fn absolute_uri_authority_stops_before_hostile_query_or_fragment_slashes() {
        for uri in [
            "https://bmc.example?next=/redfish/v1/Chassis/1/Sensors/Forged",
            "https://bmc.example#fragment/redfish/v1/Chassis/1/Sensors/Forged",
            "//bmc.example?next=/redfish/v1/Chassis/1/Sensors/Forged",
            "//bmc.example#fragment/redfish/v1/Chassis/1/Sensors/Forged",
        ] {
            assert_eq!(canonical(uri), "/", "{uri}");
            assert_eq!(sensor_scope(uri), None, "{uri}");
        }
    }

    #[test]
    fn sensor_scope_comes_from_the_parent_collection() {
        assert_eq!(
            sensor_scope("/redfish/v1/Chassis/1/Sensors/CPU0Temp"),
            Some(SensorScope::Chassis("1".to_owned()))
        );
        assert_eq!(
            sensor_scope("https://bmc/redfish/v1/PowerEquipment/RackPDUs/PDU1/Sensors/InputPower"),
            Some(SensorScope::PowerDistribution {
                collection: "rack_pdus",
                id: "PDU1".to_owned(),
            })
        );
    }

    #[test]
    fn unsupported_or_incomplete_sensor_locations_have_no_scope() {
        assert_eq!(sensor_scope("/redfish/v1/Systems/1/Sensors/CPU0"), None);
        assert_eq!(sensor_scope("/redfish/v1/Chassis"), None);
        assert_eq!(sensor_scope("/redfish/v1/Chassis/1/Sensors"), None);
    }

    #[test]
    fn sensor_scope_requires_the_exact_path_shape() {
        assert_eq!(
            sensor_scope("/redfish//v1//Chassis//1//Sensors//CPU0//"),
            Some(SensorScope::Chassis("1".to_owned()))
        );
        for uri in [
            "/redfish/v2/Chassis/1/Sensors/CPU0",
            "/redfish/v1/Chassis/1/Sensors/CPU0/Reading",
            "/redfish/v1/Chassis/1/Actuators/CPU0",
            "/redfish/v1/PowerEquipment/Unknown/PDU1/Sensors/InputPower",
            "/redfish/v1/PowerEquipment/RackPDUs/PDU1/Sensors",
        ] {
            assert_eq!(sensor_scope(uri), None, "{uri}");
        }
    }
}
