// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use nv_telemetry_core::Attributes;
use nv_telemetry_core::Name;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::Reading;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::Timestamp;

use crate::uri::canonical;

/// Canonical identity shared by all Redfish acquisition routes for a signal.
///
/// A sensor resource and a metric report name the same reading with different
/// URIs, so every conversion reduces its input to the resource denoted. That
/// happens here rather than at the call sites because a key that skipped it
/// would silently fail to join.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalKey(Name);

impl SignalKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Takes an owned URI, keeping its allocation when nothing has to go.
    ///
    /// Only a metric report names a reading with a fragment, so most inputs
    /// are already canonical and reducing one must not cost a copy.
    fn from_owned(value: impl Into<Name> + AsRef<str>) -> Self {
        let reduced = canonical(value.as_ref());
        if reduced.len() == value.as_ref().len() {
            return Self(value.into());
        }
        Self(Name::from(reduced))
    }
}

impl From<Name> for SignalKey {
    fn from(value: Name) -> Self {
        Self::from_owned(value)
    }
}

impl From<String> for SignalKey {
    fn from(value: String) -> Self {
        Self::from_owned(value)
    }
}

impl From<&str> for SignalKey {
    fn from(value: &str) -> Self {
        Self(Name::from(canonical(value)))
    }
}

/// Metadata projection result ready to be installed in a [`SignalCatalog`].
#[derive(Clone, Debug, PartialEq)]
pub struct SignalDescriptorRecord {
    key: SignalKey,
    descriptor: SignalDescriptor,
}

impl SignalDescriptorRecord {
    pub fn new(key: impl Into<SignalKey>, descriptor: SignalDescriptor) -> Self {
        Self {
            key: key.into(),
            descriptor,
        }
    }

    pub const fn key(&self) -> &SignalKey {
        &self.key
    }

    pub const fn descriptor(&self) -> &SignalDescriptor {
        &self.descriptor
    }

    pub fn into_parts(self) -> (SignalKey, SignalDescriptor) {
        (self.key, self.descriptor)
    }
}

/// A source reading that has not yet been joined with signal metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalSample {
    source_key: Name,
    signal_key: SignalKey,
    value: NumericValue,
    observed_at: Option<Timestamp>,
    attributes: Attributes,
    reported_state: Option<ReportedState>,
}

impl SignalSample {
    pub fn new(
        source_key: impl Into<Name>,
        signal_key: impl Into<SignalKey>,
        value: impl Into<NumericValue>,
    ) -> Self {
        Self {
            source_key: source_key.into(),
            signal_key: signal_key.into(),
            value: value.into(),
            observed_at: None,
            attributes: Attributes::empty(),
            reported_state: None,
        }
    }

    pub const fn signal_key(&self) -> &SignalKey {
        &self.signal_key
    }

    #[must_use]
    pub fn with_observed_at(mut self, observed_at: Timestamp) -> Self {
        self.observed_at = Some(observed_at);
        self
    }

    #[must_use]
    pub fn with_attributes(mut self, attributes: Attributes) -> Self {
        self.attributes = attributes;
        self
    }

    #[must_use]
    pub fn with_reported_state(mut self, reported_state: ReportedState) -> Self {
        self.reported_state = Some(reported_state);
        self
    }

    fn into_reading(self, descriptor: Arc<SignalDescriptor>) -> Reading {
        let mut reading =
            Reading::new(self.source_key, descriptor, self.value).with_attributes(self.attributes);
        if let Some(observed_at) = self.observed_at {
            reading = reading.with_observed_at(observed_at);
        }
        if let Some(reported_state) = self.reported_state {
            reading = reading.with_reported_state(reported_state);
        }
        reading
    }
}

/// Outcome of installing a metadata projection into a [`SignalCatalog`].
#[derive(Clone, Debug, PartialEq)]
pub enum SignalUpdate {
    /// The signal was not previously known.
    Added(Arc<SignalDescriptor>),
    /// The definition changed and was replaced at a new revision.
    Revised {
        descriptor: Arc<SignalDescriptor>,
        previous: Arc<SignalDescriptor>,
    },
    /// The refresh reported the same definition, so the existing one is kept.
    Unchanged(Arc<SignalDescriptor>),
}

impl SignalUpdate {
    pub fn descriptor(&self) -> &Arc<SignalDescriptor> {
        match self {
            Self::Added(descriptor)
            | Self::Revised { descriptor, .. }
            | Self::Unchanged(descriptor) => descriptor,
        }
    }
}

/// A catalog slot: the shared definition plus when it was last re-observed.
///
/// The confirmation time lives here rather than on the descriptor because an
/// unchanged refresh must not replace the descriptor that readings share.
#[derive(Clone, Debug)]
struct CatalogEntry {
    descriptor: Arc<SignalDescriptor>,
    last_confirmed_at: Timestamp,
}

/// Immutable signal metadata indexed by canonical logical identity.
#[derive(Clone, Debug, Default)]
pub struct SignalCatalog {
    signals: BTreeMap<SignalKey, CatalogEntry>,
}

impl SignalCatalog {
    pub const fn new() -> Self {
        Self {
            signals: BTreeMap::new(),
        }
    }

    /// Installs a metadata projection, preserving the existing descriptor when
    /// the refresh reports an unchanged definition.
    ///
    /// Revisions are assigned here rather than by the projection, because only
    /// the catalog can see whether anything changed.
    pub fn upsert(&mut self, record: SignalDescriptorRecord) -> SignalUpdate {
        use std::collections::btree_map::Entry;

        let (key, descriptor) = record.into_parts();
        let confirmed_at = descriptor.observed_at;
        match self.signals.entry(key) {
            Entry::Occupied(mut slot) => {
                let entry = slot.get_mut();
                entry.last_confirmed_at = confirmed_at;
                if entry.descriptor.matches_definition(&descriptor) {
                    return SignalUpdate::Unchanged(Arc::clone(&entry.descriptor));
                }
                let descriptor = Arc::new(descriptor.with_revision(entry.descriptor.revision + 1));
                let previous = std::mem::replace(&mut entry.descriptor, Arc::clone(&descriptor));
                SignalUpdate::Revised {
                    descriptor,
                    previous,
                }
            }
            Entry::Vacant(slot) => {
                let descriptor = Arc::new(descriptor.with_revision(0));
                slot.insert(CatalogEntry {
                    descriptor: Arc::clone(&descriptor),
                    last_confirmed_at: confirmed_at,
                });
                SignalUpdate::Added(descriptor)
            }
        }
    }

    pub fn get(&self, key: &SignalKey) -> Option<&Arc<SignalDescriptor>> {
        self.signals.get(key).map(|entry| &entry.descriptor)
    }

    /// Returns when this signal's definition was last re-observed.
    ///
    /// Advances on every refresh, including one that changed nothing, whereas
    /// the descriptor's `observed_at` marks when the definition first appeared.
    pub fn last_confirmed_at(&self, key: &SignalKey) -> Option<Timestamp> {
        self.signals.get(key).map(|entry| entry.last_confirmed_at)
    }

    /// Drops signals not re-observed since `cutoff`.
    ///
    /// A sensor removed from an endpoint stops appearing in metadata sweeps but
    /// leaves its descriptor behind.
    pub fn retain_confirmed_since(&mut self, cutoff: Timestamp) {
        self.signals
            .retain(|_, entry| entry.last_confirmed_at >= cutoff);
    }

    pub fn len(&self) -> usize {
        self.signals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    /// Attaches the catalogued descriptor to a sample to produce a reading.
    ///
    /// # Errors
    ///
    /// Returns [`UnresolvedSignal`] if the sample names a signal whose metadata
    /// has not been catalogued, which happens when a metric report arrives
    /// before the resource defining the signal has been read.
    pub fn resolve(&self, sample: SignalSample) -> Result<Reading, UnresolvedSignal> {
        let descriptor = self
            .signals
            .get(sample.signal_key())
            .map(|entry| Arc::clone(&entry.descriptor))
            .ok_or_else(|| UnresolvedSignal {
                key: sample.signal_key().clone(),
            })?;
        Ok(sample.into_reading(descriptor))
    }
}

/// A sample referenced metadata not present in the current catalog revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedSignal {
    key: SignalKey,
}

impl UnresolvedSignal {
    pub const fn key(&self) -> &SignalKey {
        &self.key
    }
}
