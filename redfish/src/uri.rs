// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::borrow::Cow;
use std::fmt::Write;

/// The parent collection scope encoded by a Sensor URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SensorScope {
    Chassis(String),
    PowerDistribution {
        collection: &'static str,
        id: String,
    },
}

/// Reduces an absolute or relative Redfish URI to its endpoint-local path.
///
/// Scheme and authority, query, fragment, trailing separators, and empty path
/// segments do not distinguish a Redfish resource, and neither does escaping a
/// character RFC 3986 treats as unreserved. Relative paths are rooted so all
/// acquisition routes produce the same key.
///
/// Case and dot segments are left as the device wrote them, and an escape of a
/// reserved character stays escaped with its hexadecimal digits upper-cased:
/// `%2F` denotes a character inside a segment rather than a segment boundary,
/// and Redfish resource names are case-sensitive.
///
/// The output is a well-formed URI path, so canonicalizing it again returns it
/// unchanged. Every route that reaches a signal identity depends on that: a
/// key that canonicalized differently on a second pass would stop joining.
pub(crate) fn canonical(uri: &str) -> Cow<'_, str> {
    let path = endpoint_local_path(uri);
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return Cow::Borrowed("/");
    }
    if path.starts_with('/') && !path.contains("//") && !path.contains('%') {
        return Cow::Borrowed(path);
    }
    // Trailing separators are already gone, so a non-empty path here holds at
    // least one non-empty segment.
    let mut normalized = String::with_capacity(path.len() + 1);
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        normalized.push('/');
        push_normalized_segment(segment, &mut normalized);
    }
    Cow::Owned(normalized)
}

/// Strips the authority of an absolute URI, leaving a relative reference whole.
///
/// A leading `//` is only an authority once a scheme has introduced one. On a
/// relative reference it is an empty leading segment, which is what a device
/// concatenating a base and a path emits, and dropping the segment after it
/// would move the resource to a path it does not occupy.
fn endpoint_local_path(uri: &str) -> &str {
    match authority_after_scheme(uri) {
        Some(authority) => path_after_authority(authority),
        None => uri,
    }
}

/// Returns the authority following `scheme://`, if the URI has a scheme.
///
/// A `://` further along the URI belongs to a query string or fragment: no
/// scheme character is a path, query, or fragment delimiter, so requiring a
/// valid scheme before the separator also requires the separator to come
/// first.
fn authority_after_scheme(uri: &str) -> Option<&str> {
    let (scheme, authority) = uri.split_once("://")?;
    is_scheme(scheme).then_some(authority)
}

fn is_scheme(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn path_after_authority(authority: &str) -> &str {
    let Some(index) = authority.find(['/', '?', '#']) else {
        return "/";
    };
    let path = &authority[index..];
    if path.starts_with('/') {
        path
    } else {
        "/"
    }
}

/// Copies one path segment, decoding escapes of unreserved characters and
/// upper-casing the hexadecimal digits of the rest.
///
/// Only unreserved escapes are decoded: decoding a reserved one would change
/// what the path says, since `%2F` inside a segment is not a boundary between
/// segments.
///
/// A `%` that does not introduce two hexadecimal digits is not an escape, and
/// is re-escaped rather than copied. Copying it through would leave the output
/// malformed, so a following escape could pair with it into an escape the
/// device never wrote: `%4%41` would become `%4A` and then `J`, and `%2%41`
/// would become `%2A`, which is how a device spells a sensor named `*`.
fn push_normalized_segment(segment: &str, normalized: &mut String) {
    let mut rest = segment;
    while let Some(offset) = rest.find('%') {
        normalized.push_str(&rest[..offset]);
        rest = &rest[offset..];
        match escape_at(rest) {
            Some(byte) if is_unreserved(byte) => {
                normalized.push(char::from(byte));
                rest = &rest[ESCAPE_LEN..];
            }
            Some(byte) => {
                let _ = write!(normalized, "%{byte:02X}");
                rest = &rest[ESCAPE_LEN..];
            }
            None => {
                normalized.push_str("%25");
                rest = &rest[1..];
            }
        }
    }
    normalized.push_str(rest);
}

/// Length of `%` followed by two hexadecimal digits.
const ESCAPE_LEN: usize = 3;

/// Returns the byte denoted by the escape at the start of `text`.
///
/// Each digit is read on its own rather than through `u8::from_str_radix`,
/// which accepts a leading sign and so would decode `%+1`.
fn escape_at(text: &str) -> Option<u8> {
    let &[high, low] = text.as_bytes().get(1..ESCAPE_LEN)? else {
        return None;
    };
    Some((hex_digit(high)? << 4) | hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

/// Reports whether RFC 3986 treats a byte as equivalent escaped or unescaped.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Returns the supported parent collection scope encoded by a Sensor URI.
#[cfg(test)]
pub(crate) fn sensor_scope(uri: &str) -> Option<SensorScope> {
    let canonical = canonical(uri);
    sensor_scope_from_canonical(&canonical)
}

/// Returns the supported parent collection scope encoded by a canonical path.
///
/// The service root is mandatory: DSP0266 gives every `@odata.id` as an
/// absolute path under `/redfish/v1`, so a path that names a collection
/// without it is not a location this crate recognises. Accepting one would let
/// `//Chassis/1/Sensors/CPU0Temp`, which canonicalizes to
/// `/Chassis/1/Sensors/CPU0Temp`, claim the subject of the sensor at
/// `/redfish/v1/Chassis/1/Sensors/CPU0Temp` while keeping a source key of its
/// own, and two resources claiming one subject cost the whole graph.
///
/// A path holding a `.` or `..` segment has no scope. Canonicalization does
/// not resolve dot segments, so treating one as an ordinary name would let
/// `%2E%2E` reach a subject id as a traversal the device never spelled, while
/// the literal `..` reached the same one; a Redfish `@odata.id` has neither.
pub(crate) fn sensor_scope_from_canonical(canonical: &str) -> Option<SensorScope> {
    if canonical.split('/').any(is_dot_segment) {
        return None;
    }
    let mut segments = canonical.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != "redfish" || segments.next()? != "v1" {
        return None;
    }
    let resource = segments.next()?;

    match resource {
        "Chassis" => {
            let parent = segments.next()?;
            if !names_one_sensor(&mut segments) {
                return None;
            }
            Some(SensorScope::Chassis(parent.to_owned()))
        }
        "PowerEquipment" => {
            let collection = power_distribution_collection(segments.next()?)?;
            let parent = segments.next()?;
            if !names_one_sensor(&mut segments) {
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

/// Reports whether the remaining segments name one member of a `Sensors`
/// collection and nothing below it.
fn names_one_sensor<'a>(segments: &mut impl Iterator<Item = &'a str>) -> bool {
    segments.next() == Some("Sensors") && segments.next().is_some() && segments.next().is_none()
}

fn is_dot_segment(segment: &str) -> bool {
    matches!(segment, "." | "..")
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
        ] {
            assert_eq!(canonical(uri), "/", "{uri}");
            assert_eq!(sensor_scope(uri), None, "{uri}");
        }
    }

    #[test]
    fn a_leading_double_slash_without_a_scheme_keeps_every_segment() {
        assert_eq!(
            canonical("//redfish/v1/Chassis/1/Sensors/CPU0Temp"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
            "a base concatenated onto a rooted path is an empty leading segment"
        );
        assert_eq!(
            sensor_scope("//redfish/v1/Chassis/1/Sensors/CPU0Temp"),
            Some(SensorScope::Chassis("1".to_owned()))
        );
        // The first segment is a name, so a host smuggled into one is not a
        // location this crate recognises.
        assert_eq!(
            canonical("//bmc.example/redfish/v1/Chassis/1/Sensors/Forged"),
            "/bmc.example/redfish/v1/Chassis/1/Sensors/Forged"
        );
        assert_eq!(
            sensor_scope("//bmc.example/redfish/v1/Chassis/1/Sensors/Forged"),
            None
        );
    }

    #[test]
    fn a_separator_inside_a_query_or_fragment_is_not_a_scheme() {
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp?target=http://peer/x"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU0Temp#http://peer/x"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            sensor_scope("/redfish/v1/Chassis/1/Sensors/CPU0Temp?target=http://peer/x"),
            Some(SensorScope::Chassis("1".to_owned()))
        );
        assert_eq!(
            canonical("1nvalid://bmc.example/redfish/v1/Chassis/1/Sensors/CPU0Temp"),
            "/1nvalid:/bmc.example/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
    }

    #[test]
    fn empty_segments_do_not_split_one_sensor_into_two_identities() {
        assert_eq!(
            canonical("/redfish/v1//Chassis/1/Sensors/CPU0Temp"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("redfish//v1//Chassis//1//Sensors//CPU0Temp//"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
    }

    #[test]
    fn escaping_a_character_that_needs_no_escape_names_the_same_resource() {
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU%30Temp"),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU%7eTemp"),
            "/redfish/v1/Chassis/1/Sensors/CPU~Temp"
        );
        // A reserved escape stays escaped, because the character it denotes
        // would otherwise be read as a path separator.
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU%2fTemp"),
            "/redfish/v1/Chassis/1/Sensors/CPU%2FTemp"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/100%"),
            "/redfish/v1/Chassis/1/Sensors/100%25"
        );
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/CPU%2GTemp"),
            "/redfish/v1/Chassis/1/Sensors/CPU%252GTemp"
        );
    }

    #[test]
    fn a_stray_percent_cannot_pair_with_the_escape_that_follows_it() {
        // A stray `%` is escaped where it stands, so the digits after it
        // cannot complete the escape they would otherwise spell: neither of
        // these reaches `%4A` or the `%2A` of a sensor named `*`.
        assert_eq!(
            canonical("/redfish/v1/Chassis/1/Sensors/%4%41"),
            "/redfish/v1/Chassis/1/Sensors/%254A"
        );
        assert_ne!(
            canonical("/redfish/v1/Chassis/1/Sensors/%2%41"),
            canonical("/redfish/v1/Chassis/1/Sensors/%2A")
        );
    }

    #[test]
    fn canonicalizing_a_canonical_path_changes_nothing() {
        for uri in [
            "/redfish/v1/Chassis/1/Sensors/%4%41",
            "/redfish/v1/Chassis/1/Sensors/%2%41",
            "/redfish/v1/Chassis/1/Sensors/%",
            "/redfish/v1/Chassis/1/Sensors/%A",
            "/redfish/v1/Chassis/1/Sensors/%ZZ",
            "/redfish/v1/Chassis/1/Sensors/%%41",
            "/redfish/v1/Chassis/1/Sensors/%25",
            "/redfish/v1/Chassis/1/Sensors/%2F",
            "/redfish/v1/Chassis/1/Sensors/%2A",
            "/redfish/v1/Chassis/1/Sensors/%+1",
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp%",
            "/redfish/v1/Chassis/1/Sensors/%E2%82%AC",
            "//redfish/v1/Chassis/1/Sensors/CPU%30Temp/",
            "https://bmc.example/redfish/v1/Chassis/1/Sensors/CPU%7eTemp?x=1#y",
        ] {
            let once = canonical(uri).into_owned();
            assert_eq!(canonical(&once), once.as_str(), "{uri}");
        }
    }

    #[test]
    fn a_dot_segment_has_no_scope_however_the_device_spelled_it() {
        for uri in [
            "/redfish/v1/Chassis/%2E%2E/Sensors/CPU0Temp",
            "/redfish/v1/Chassis/../Sensors/CPU0Temp",
            "/redfish/v1/Chassis/1/Sensors/%2E",
            "/redfish/v1/Chassis/1/Sensors/.",
        ] {
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
    fn the_service_root_prefix_is_mandatory() {
        // A path one prefix short reads its first segment as the collection,
        // so it would reach the subject of the sensor rooted properly at that
        // collection while keeping a source key of its own.
        assert_eq!(sensor_scope("//Chassis/1/Sensors/CPU0Temp"), None);
        assert_eq!(sensor_scope("Chassis/1/Sensors/CPU0Temp"), None);
        assert_eq!(sensor_scope("/Chassis/1/Sensors/CPU0Temp"), None);
        assert_eq!(
            sensor_scope("/PowerEquipment/RackPDUs/PDU1/Sensors/InputPower"),
            None
        );
        assert_eq!(sensor_scope("/v1/Chassis/1/Sensors/CPU0Temp"), None);
        assert_eq!(sensor_scope("/redfish/Chassis/1/Sensors/CPU0Temp"), None);
    }

    #[test]
    fn the_service_root_is_recognized_however_the_device_rooted_it() {
        for uri in [
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
            "//redfish/v1/Chassis/1/Sensors/CPU0Temp",
            "redfish/v1/Chassis/1/Sensors/CPU0Temp",
        ] {
            assert_eq!(
                sensor_scope(uri),
                Some(SensorScope::Chassis("1".to_owned())),
                "{uri}"
            );
        }
    }

    #[test]
    fn unsupported_or_incomplete_sensor_locations_have_no_scope() {
        assert_eq!(sensor_scope("/redfish/v1/Systems/1/Sensors/CPU0"), None);
        assert_eq!(sensor_scope("/redfish/v1/Chassis"), None);
        assert_eq!(sensor_scope("/redfish/v1/Chassis/1/Sensors"), None);
    }

    #[test]
    fn sensor_scope_requires_the_exact_path_shape() {
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
