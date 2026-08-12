// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The one accepted grammar for requested-location identity patterns.
//!
//! Manifests carry strings because protobuf does. Past this boundary the
//! compiler carries [`LocationPattern`], so lint, lowering, and emission
//! cannot disagree about normalization or placeholder segmentation.

/// A canonical absolute resource path with one named capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocationPattern {
    template: String,
    capture: String,
    segments: Vec<LocationSegment>,
}

impl LocationPattern {
    /// Parses exactly the path grammar the generated matcher implements.
    pub(crate) fn parse(template: &str, capture: &str) -> Result<Self, String> {
        if capture.is_empty()
            || capture
                .chars()
                .any(|character| matches!(character, '{' | '}' | '/' | '?' | '#'))
        {
            return Err("capture names must be non-empty placeholder names".to_owned());
        }
        if !template.starts_with('/') {
            return Err("Redfish locations are absolute paths".to_owned());
        }
        if template.contains('?') || template.contains('#') {
            return Err(
                "query and fragment components are not part of a resource-path template".to_owned(),
            );
        }
        if template.len() > 1 && template.ends_with('/') {
            return Err("resource-path templates have no trailing separator".to_owned());
        }

        let mut captures = 0usize;
        let mut segments = Vec::new();
        for segment in template[1..].split('/') {
            if segment.is_empty() {
                return Err("resource-path templates have no empty segments".to_owned());
            }
            if matches!(segment, "." | "..") {
                return Err(format!("path segment `{segment}` is not canonical"));
            }
            let has_brace = segment.contains('{') || segment.contains('}');
            if !has_brace {
                segments.push(LocationSegment::Literal(segment.to_owned()));
                continue;
            }
            let Some(name) = segment
                .strip_prefix('{')
                .and_then(|segment| segment.strip_suffix('}'))
            else {
                return Err(format!(
                    "placeholder segment `{segment}` must be exactly `{{name}}`"
                ));
            };
            if name.is_empty() {
                return Err("placeholder names are non-empty".to_owned());
            }
            if name.contains('{') || name.contains('}') {
                return Err(format!(
                    "placeholder segment `{segment}` contains nested braces"
                ));
            }
            if name == capture {
                captures += 1;
                segments.push(LocationSegment::Capture);
            } else {
                segments.push(LocationSegment::Wildcard);
            }
        }

        match captures {
            1 => Ok(Self {
                template: template.to_owned(),
                capture: capture.to_owned(),
                segments,
            }),
            0 => Err(format!(
                "capture `{capture}` is not a complete placeholder segment"
            )),
            count => Err(format!("capture `{capture}` appears {count} times")),
        }
    }

    pub(crate) fn template(&self) -> &str {
        &self.template
    }

    pub(crate) fn capture(&self) -> &str {
        &self.capture
    }

    pub(crate) fn segments(&self) -> &[LocationSegment] {
        &self.segments
    }
}

/// One already-validated matcher operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocationSegment {
    Literal(String),
    Wildcard,
    Capture,
}

#[cfg(test)]
mod tests {
    use super::LocationPattern;

    #[test]
    fn a_canonical_pattern_is_typed_once() {
        let pattern =
            LocationPattern::parse("/redfish/v1/Chassis/{chassis}/Sensors/{id}", "chassis")
                .expect("the shipped grammar is canonical");

        assert_eq!(
            pattern.template(),
            "/redfish/v1/Chassis/{chassis}/Sensors/{id}"
        );
        assert_eq!(pattern.capture(), "chassis");
        assert_eq!(pattern.segments().len(), 6);
    }

    #[test]
    fn normalization_cannot_change_an_accepted_pattern() {
        for template in [
            "/redfish/v1/Chassis/{chassis}/Sensors/{id}/",
            "/redfish/v1/Chassis/{chassis}/Sensors/{id}?view=full",
            "/redfish/v1/Chassis/{chassis}/Sensors/{id}#/Reading",
            "/redfish/v1//Chassis/{chassis}/Sensors/{id}",
            "/redfish/v1/../Chassis/{chassis}/Sensors/{id}",
        ] {
            assert!(
                LocationPattern::parse(template, "chassis").is_err(),
                "{template} must not survive into emission"
            );
        }
    }
}
