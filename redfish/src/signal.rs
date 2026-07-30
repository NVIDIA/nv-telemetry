// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use nv_telemetry_core::Attributes;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::Reading;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::SourceKey;
use nv_telemetry_core::Timestamp;

use crate::uri::canonical;

/// Endpoint-local identity shared by all Redfish acquisition routes.
///
/// A sensor resource and a metric report name the same reading with different
/// URIs, so every conversion reduces its input to the resource denoted. That
/// happens here rather than at the call sites because a key that skipped it
/// would silently fail to join.
///
/// A key does not contain an endpoint identity. It is meaningful only inside
/// the [`SignalCatalog`] belonging to the endpoint that produced it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalKey(SourceKey);

impl SignalKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) const fn from_canonical_source(value: SourceKey) -> Self {
        Self(value)
    }

    fn from_owned(value: String) -> Self {
        let replacement = {
            let canonical = canonical(&value);
            (canonical.as_ref() != value).then(|| canonical.into_owned())
        };
        Self(SourceKey::from(replacement.unwrap_or(value)))
    }
}

impl From<SourceKey> for SignalKey {
    fn from(value: SourceKey) -> Self {
        let replacement = {
            let canonical = canonical(value.as_str());
            (canonical.as_ref() != value.as_str()).then(|| canonical.into_owned())
        };
        replacement.map_or_else(|| Self(value), |value| Self(SourceKey::from(value)))
    }
}

impl From<String> for SignalKey {
    fn from(value: String) -> Self {
        Self::from_owned(value)
    }
}

impl From<&str> for SignalKey {
    fn from(value: &str) -> Self {
        Self(match canonical(value) {
            std::borrow::Cow::Borrowed(value) => SourceKey::from(value),
            std::borrow::Cow::Owned(value) => SourceKey::from(value),
        })
    }
}

impl fmt::Display for SignalKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Metadata projection result ready to be installed in a [`SignalCatalog`].
#[derive(Clone, Debug, PartialEq, Eq)]
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

    pub(crate) const fn from_canonical_source(
        key: SourceKey,
        descriptor: SignalDescriptor,
    ) -> Self {
        Self {
            key: SignalKey::from_canonical_source(key),
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSample {
    source_key: SourceKey,
    signal_key: SignalKey,
    value: NumericValue,
    observed_at: Option<Timestamp>,
    attributes: Attributes,
    reported_state: Option<ReportedState>,
}

impl SignalSample {
    pub fn new(
        source_key: impl Into<SourceKey>,
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

    pub(crate) fn from_canonical_source(
        source_key: SourceKey,
        value: impl Into<NumericValue>,
    ) -> Self {
        Self {
            signal_key: SignalKey::from_canonical_source(source_key.clone()),
            source_key,
            value: value.into(),
            observed_at: None,
            attributes: Attributes::empty(),
            reported_state: None,
        }
    }

    pub const fn signal_key(&self) -> &SignalKey {
        &self.signal_key
    }

    pub const fn source_key(&self) -> &SourceKey {
        &self.source_key
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
    /// Metadata older than the installed confirmation was ignored.
    Stale {
        descriptor: Arc<SignalDescriptor>,
        rejected: SignalDescriptor,
    },
}

impl SignalUpdate {
    pub fn descriptor(&self) -> &Arc<SignalDescriptor> {
        match self {
            Self::Added(descriptor)
            | Self::Revised { descriptor, .. }
            | Self::Unchanged(descriptor)
            | Self::Stale { descriptor, .. } => descriptor,
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

/// Immutable signal metadata for one Redfish endpoint.
///
/// URI keys are normalized paths and deliberately omit host identity. Do not
/// share one catalog across endpoints: identical paths on two BMCs name
/// different signals. A caller should create one catalog per endpoint and
/// remove entries after endpoint-local discovery sweeps report disappearance.
#[derive(Clone, Debug)]
pub struct SignalCatalog {
    signals: BTreeMap<SignalKey, CatalogEntry>,
    max_signals: usize,
}

impl SignalCatalog {
    /// Default endpoint-local bound, high enough for unusually large BMCs
    /// while preventing an unbounded stream of identities from growing memory.
    pub const DEFAULT_MAX_SIGNALS: usize = 100_000;

    pub const fn new() -> Self {
        Self::with_max_signals(Self::DEFAULT_MAX_SIGNALS)
    }

    pub const fn with_max_signals(max_signals: usize) -> Self {
        Self {
            signals: BTreeMap::new(),
            max_signals,
        }
    }

    /// Installs a metadata projection, preserving the existing descriptor when
    /// the refresh reports an unchanged definition.
    ///
    /// Revisions are assigned here rather than by the projection, because only
    /// the catalog can see whether anything changed.
    /// # Errors
    ///
    /// Returns [`SignalCatalogError::Full`] if a new identity would exceed the
    /// configured bound, or [`SignalCatalogError::RevisionExhausted`] if an
    /// existing signal has already consumed every `u64` revision. Updating an
    /// existing identity remains possible at catalog capacity.
    pub fn upsert(
        &mut self,
        record: SignalDescriptorRecord,
    ) -> Result<SignalUpdate, SignalCatalogError> {
        use std::collections::btree_map::Entry;

        let (key, descriptor) = record.into_parts();
        let confirmed_at = descriptor.observed_at;
        let at_capacity = self.signals.len() >= self.max_signals;
        match self.signals.entry(key) {
            Entry::Occupied(mut slot) => {
                let key = slot.key().clone();
                let entry = slot.get_mut();
                if confirmed_at < entry.last_confirmed_at
                    || (confirmed_at == entry.last_confirmed_at
                        && !entry.descriptor.matches_definition(&descriptor))
                {
                    return Ok(SignalUpdate::Stale {
                        descriptor: Arc::clone(&entry.descriptor),
                        rejected: descriptor,
                    });
                }
                if entry.descriptor.matches_definition(&descriptor) {
                    entry.last_confirmed_at = confirmed_at;
                    return Ok(SignalUpdate::Unchanged(Arc::clone(&entry.descriptor)));
                }
                let Some(revision) = entry.descriptor.revision.checked_add(1) else {
                    return Err(SignalCatalogError::RevisionExhausted(
                        SignalRevisionExhausted {
                            record: Box::new(SignalDescriptorRecord { key, descriptor }),
                        },
                    ));
                };
                entry.last_confirmed_at = confirmed_at;
                let descriptor = Arc::new(descriptor.with_revision(revision));
                let previous = std::mem::replace(&mut entry.descriptor, Arc::clone(&descriptor));
                Ok(SignalUpdate::Revised {
                    descriptor,
                    previous,
                })
            }
            Entry::Vacant(slot) => {
                if at_capacity {
                    return Err(SignalCatalogError::Full(SignalCatalogFull {
                        limit: self.max_signals,
                        record: Box::new(SignalDescriptorRecord {
                            key: slot.into_key(),
                            descriptor,
                        }),
                    }));
                }
                let descriptor = Arc::new(descriptor.with_revision(0));
                slot.insert(CatalogEntry {
                    descriptor: Arc::clone(&descriptor),
                    last_confirmed_at: confirmed_at,
                });
                Ok(SignalUpdate::Added(descriptor))
            }
        }
    }

    pub fn get(&self, key: &SignalKey) -> Option<&Arc<SignalDescriptor>> {
        self.signals.get(key).map(|entry| &entry.descriptor)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SignalKey, &Arc<SignalDescriptor>)> {
        self.signals
            .iter()
            .map(|(key, entry)| (key, &entry.descriptor))
    }

    pub fn remove(&mut self, key: &SignalKey) -> Option<Arc<SignalDescriptor>> {
        self.signals.remove(key).map(|entry| entry.descriptor)
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
    pub fn retain_confirmed_since(&mut self, cutoff: Timestamp) -> usize {
        let previous = self.signals.len();
        self.signals
            .retain(|_, entry| entry.last_confirmed_at >= cutoff);
        previous - self.signals.len()
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
        match self.signals.get(sample.signal_key()) {
            Some(entry) => Ok(sample.into_reading(Arc::clone(&entry.descriptor))),
            None => Err(UnresolvedSignal {
                sample: Box::new(sample),
            }),
        }
    }
}

impl Default for SignalCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// A signal metadata update the catalog could not install.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalCatalogError {
    Full(SignalCatalogFull),
    RevisionExhausted(SignalRevisionExhausted),
}

impl fmt::Display for SignalCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(error) => error.fmt(formatter),
            Self::RevisionExhausted(error) => error.fmt(formatter),
        }
    }
}

impl Error for SignalCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Full(error) => Some(error),
            Self::RevisionExhausted(error) => Some(error),
        }
    }
}

/// A new signal would exceed an endpoint catalog's configured bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalCatalogFull {
    limit: usize,
    record: Box<SignalDescriptorRecord>,
}

impl SignalCatalogFull {
    pub const fn limit(&self) -> usize {
        self.limit
    }

    pub fn record(&self) -> &SignalDescriptorRecord {
        &self.record
    }

    pub fn into_record(self) -> SignalDescriptorRecord {
        *self.record
    }
}

impl fmt::Display for SignalCatalogFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "signal catalog limit of {} entries exceeded by '{}'",
            self.limit,
            self.record.key()
        )
    }
}

impl Error for SignalCatalogFull {}

/// A signal definition changed after its revision counter was exhausted.
///
/// The rejected record is retained so the caller can log, retry, or route it
/// without reconstructing device input. The installed descriptor is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalRevisionExhausted {
    record: Box<SignalDescriptorRecord>,
}

impl SignalRevisionExhausted {
    pub fn record(&self) -> &SignalDescriptorRecord {
        &self.record
    }

    pub fn into_record(self) -> SignalDescriptorRecord {
        *self.record
    }
}

impl fmt::Display for SignalRevisionExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "signal revision counter exhausted for '{}'",
            self.record.key()
        )
    }
}

impl Error for SignalRevisionExhausted {}

/// A sample referenced metadata not present in the current catalog revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedSignal {
    sample: Box<SignalSample>,
}

impl UnresolvedSignal {
    pub const fn key(&self) -> &SignalKey {
        self.sample.signal_key()
    }

    pub fn sample(&self) -> &SignalSample {
        &self.sample
    }

    pub fn into_sample(self) -> SignalSample {
        *self.sample
    }
}

impl fmt::Display for UnresolvedSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "signal metadata is not catalogued for '{}'",
            self.key()
        )
    }
}

impl Error for UnresolvedSignal {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nv_telemetry_core::Finite;
    use nv_telemetry_core::Instance;
    use nv_telemetry_core::Metric;
    use nv_telemetry_core::ReadingKind;
    use nv_telemetry_core::SourceKey;
    use nv_telemetry_core::Subject;
    use nv_telemetry_core::Unit;

    use super::SignalCatalog;
    use super::SignalCatalogError;
    use super::SignalDescriptorRecord;
    use super::SignalKey;
    use super::SignalSample;
    use super::SignalUpdate;
    use super::Timestamp;

    fn record(key: &str, observed_at: i64, unit: &'static str) -> SignalDescriptorRecord {
        let descriptor = super::SignalDescriptor::new(
            Subject::new("sensor".into(), key.into()),
            Metric::from_static("temperature"),
            Instance::from("temperature"),
            ReadingKind::Gauge,
            Unit::from_static(unit),
            Timestamp::new(observed_at, 0).expect("valid timestamp"),
        );
        SignalDescriptorRecord::new(key, descriptor)
    }

    #[test]
    fn stale_metadata_does_not_replace_or_reconfirm_a_newer_definition() {
        let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/Temperature");
        let mut catalog = SignalCatalog::new();
        catalog
            .upsert(record(key.as_str(), 20, "Cel"))
            .expect("catalog capacity");

        let update = catalog
            .upsert(record(key.as_str(), 10, "K"))
            .expect("catalog capacity");

        assert!(matches!(update, SignalUpdate::Stale { .. }));
        assert_eq!(
            catalog
                .get(&key)
                .expect("installed descriptor")
                .unit
                .as_str(),
            "Cel"
        );
        assert_eq!(
            catalog.last_confirmed_at(&key),
            Some(Timestamp::new(20, 0).expect("valid timestamp"))
        );
    }

    #[test]
    fn catalog_limit_rejects_only_new_identities_and_returns_the_record() {
        let first = SignalKey::from("/redfish/v1/Chassis/1/Sensors/First");
        let second = SignalKey::from("/redfish/v1/Chassis/1/Sensors/Second");
        let mut catalog = SignalCatalog::with_max_signals(1);
        catalog
            .upsert(record(first.as_str(), 10, "Cel"))
            .expect("first slot is available");

        let error = catalog
            .upsert(record(second.as_str(), 10, "Cel"))
            .expect_err("a second identity exceeds the limit");
        let SignalCatalogError::Full(full) = error else {
            panic!("expected catalog capacity error");
        };
        assert_eq!(full.limit(), 1);
        assert_eq!(full.into_record().key(), &second);

        assert!(matches!(
            catalog
                .upsert(record(first.as_str(), 20, "K"))
                .expect("an existing identity remains updateable"),
            SignalUpdate::Revised { .. }
        ));
        assert_eq!(catalog.iter().count(), 1);
        assert!(catalog.remove(&first).is_some());
        assert!(catalog.is_empty());
    }

    #[test]
    fn revision_exhaustion_rejects_the_update_without_replacing_metadata() {
        let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/Temperature");
        let installed = Arc::new(
            record(key.as_str(), 10, "Cel")
                .descriptor
                .with_revision(u64::MAX),
        );
        let mut catalog = SignalCatalog::new();
        catalog.signals.insert(
            key.clone(),
            super::CatalogEntry {
                descriptor: Arc::clone(&installed),
                last_confirmed_at: installed.observed_at,
            },
        );

        let error = catalog
            .upsert(record(key.as_str(), 20, "K"))
            .expect_err("revision counter is exhausted");

        let SignalCatalogError::RevisionExhausted(error) = error else {
            panic!("expected revision exhaustion");
        };
        assert_eq!(error.record().key(), &key);
        assert!(Arc::ptr_eq(
            catalog.get(&key).expect("installed descriptor"),
            &installed
        ));
        assert_eq!(catalog.last_confirmed_at(&key), Some(installed.observed_at));
    }

    #[test]
    fn unresolved_error_retains_the_sample_for_retry() {
        let sample = SignalSample::new(
            "metric-report:thermal",
            "/redfish/v1/Chassis/1/Sensors/Unknown#/Reading",
            Finite::new(42.0).expect("finite reading"),
        );
        let expected = sample.clone();

        let error = SignalCatalog::new()
            .resolve(sample)
            .expect_err("metadata has not arrived");

        assert_eq!(error.sample(), &expected);
        assert!(error.to_string().contains(expected.signal_key().as_str()));
        assert_eq!(error.into_sample(), expected);
    }

    #[test]
    fn canonical_source_key_storage_is_reused_by_signal_key() {
        let source = SourceKey::from("/redfish/v1/Chassis/1/Sensors/Temperature".to_owned());
        let allocation = source.as_str().as_ptr();

        let signal = SignalKey::from(source);

        assert_eq!(signal.as_str().as_ptr(), allocation);
    }
}
