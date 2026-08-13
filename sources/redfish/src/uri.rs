// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Location grammar: what a Redfish URI denotes before identity is derived.
//!
//! The subject's scope comes from the *requested* location, never from the
//! payload's `@odata.id` — the plan named the URI, and the payload's claim
//! about itself is provenance, not identity. Generated projections match
//! their manifest's location template over [`canonical`]'s output; this is
//! the one hook the projection compiler expects a Redfish source crate to
//! provide.

/// Reduces a Redfish URI to the resource it denotes.
///
/// A metric report names a sensor's reading with a property fragment
/// (`.../CPU0Temp#/Reading`) while the sensor resource names itself without
/// one. Query options select a representation of that same resource. Both
/// query and fragment, followed by any trailing separator, are therefore
/// dropped before the URI is used as an identity.
pub(crate) fn canonical(uri: &str) -> &str {
    let path = uri.split_once('#').map_or(uri, |(path, _)| path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() && path.starts_with('/') {
        "/"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::canonical;

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
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp/?$select=Reading#/Reading"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(canonical("/?$select=Id"), "/");
    }
}
