// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

/// Reduces a Redfish URI to the resource it denotes.
///
/// A metric report names a sensor's reading with a property fragment
/// (`.../CPU0Temp#/Reading`) while the sensor resource names itself without
/// one. Both denote the same signal, so the fragment and any trailing
/// separator are dropped before the URI is used as an identity.
pub(crate) fn canonical(uri: &str) -> &str {
    let path = uri.split_once('#').map_or(uri, |(path, _)| path);
    path.strip_suffix('/').unwrap_or(path)
}

/// Returns the chassis a sensor URI is scoped to.
///
/// `Sensor.Id` is unique only within one chassis' sensor collection, so a
/// subject built from the id alone would collide across chassis on a
/// multi-chassis endpoint. The chassis is not carried in the sensor payload;
/// only its location states it.
pub(crate) fn chassis_of(uri: &str) -> Option<&str> {
    let mut segments = canonical(uri).split('/');
    segments.find(|segment| *segment == "Chassis")?;
    segments.next().filter(|chassis| !chassis.is_empty())
}

#[cfg(test)]
mod tests {
    use super::canonical;
    use super::chassis_of;

    #[test]
    fn a_property_fragment_names_the_same_resource_as_the_bare_uri() {
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp/"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
    }

    #[test]
    fn the_chassis_comes_from_the_location_not_the_payload() {
        assert_eq!(
            chassis_of("/redfish/v1/Chassis/1/Sensors/CPU0Temp"),
            Some("1")
        );
        assert_eq!(
            chassis_of("/redfish/v1/Chassis/Blade2/Sensors/CPU0Temp#/Reading"),
            Some("Blade2")
        );
    }

    #[test]
    fn a_uri_outside_a_chassis_yields_no_scope() {
        assert_eq!(chassis_of("/redfish/v1/Systems/1/Processors/CPU0"), None);
        assert_eq!(chassis_of("/redfish/v1/Chassis"), None);
        assert_eq!(chassis_of("/redfish/v1/Chassis/"), None);
    }
}
