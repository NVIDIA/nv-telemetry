// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Canonical order and the content-hash byte stream.
//!
//! Two traits, implemented by generation for the model and by hand for the
//! value vocabulary, both walking the same logical tree:
//!
//! [`Canonical`] is the total order canonicalization sorts by. It compares
//! hash-visible fields first, in field-number order — absent before present,
//! each value type within itself — and collection-metadata fields after all
//! of them, as a final tiebreaker. The split is what keeps both properties at
//! once: elements that tie on hash-visible fields contribute identical hash
//! streams in either order, so the tiebreaker cannot move a hash, while
//! canonical bytes stay deterministic. The rule applies recursively: a nested
//! message compares by its own visible-then-metadata order wherever it sits.
//!
//! [`Digest`] feeds the logical content — present hash-visible fields,
//! labeled by field number — into any [`Hasher`] the caller supplies. The
//! model does not pick a hash function: the standard library's default is
//! deliberately unstable across processes, and pinning a specific algorithm
//! is a policy choice that belongs to whoever stores or compares the hashes.
//! What the model owns is the byte stream, and the stream is versioned by the
//! schema it was generated from.
//!
//! The stream is injective by construction, because a hash over an ambiguous
//! serialization equates observations that differ:
//!
//! - every present field is prefixed by its field number, and field numbers
//!   are never zero, so `END` (a zero) closes each message unambiguously —
//!   without it, a nested message's fields would run into its parent's;
//! - strings, bytes, and repeated fields carry length or count prefixes, so
//!   `("ab")` and `("a", "b")` cannot collide;
//! - fixed-width scalars are written little-endian at full width, and a
//!   `Value` is prefixed by its arm's field number, so an integer 5 and a
//!   double 5.0 are different bytes;
//! - absent fields contribute nothing at all, which is what lets a schema
//!   revision add fields without moving the hash of data that does not use
//!   them.
//!
//! Encoded protobuf bytes are never fed to the hash: wire encoding is not
//! canonical across implementations, and unknown fields would make equal
//! content hash unequal.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::hash::Hasher;

use crate::Value;

/// The canonical total order: hash-visible fields in field-number order,
/// then collection metadata as the tiebreaker.
pub(crate) trait Canonical {
    fn canonical_cmp(&self, other: &Self) -> Ordering;
}

/// Feeds logical content — hash-visible fields only — into a hasher.
pub(crate) trait Digest {
    fn digest<H: Hasher>(&self, state: &mut H);
}

/// Closes a message's field list. Field numbers are never zero, so a zero
/// tag cannot be confused with a field.
pub(crate) fn end<H: Hasher>(state: &mut H) {
    state.write(&0u32.to_le_bytes());
}

/// Labels the value that follows with its field number.
pub(crate) fn tag<H: Hasher>(state: &mut H, field: u32) {
    state.write(&field.to_le_bytes());
}

/// A count or byte length, full width so no value can masquerade as one.
pub(crate) fn count<H: Hasher>(state: &mut H, length: usize) {
    state.write(&(length as u64).to_le_bytes());
}

pub(crate) fn str_value<H: Hasher>(state: &mut H, value: &str) {
    count(state, value.len());
    state.write(value.as_bytes());
}

pub(crate) fn bytes_value<H: Hasher>(state: &mut H, value: &[u8]) {
    count(state, value.len());
    state.write(value);
}

pub(crate) fn bool_value<H: Hasher>(state: &mut H, value: bool) {
    state.write(&[u8::from(value)]);
}

pub(crate) fn i64_value<H: Hasher>(state: &mut H, value: i64) {
    state.write(&value.to_le_bytes());
}

pub(crate) fn u64_value<H: Hasher>(state: &mut H, value: u64) {
    state.write(&value.to_le_bytes());
}

pub(crate) fn u32_value<H: Hasher>(state: &mut H, value: u32) {
    state.write(&value.to_le_bytes());
}

pub(crate) fn i32_value<H: Hasher>(state: &mut H, value: i32) {
    state.write(&value.to_le_bytes());
}

/// A finite double, by bits: construction normalized the zeros, so equal
/// values are equal bits.
pub(crate) fn f64_value<H: Hasher>(state: &mut H, value: f64) {
    state.write(&value.to_bits().to_le_bytes());
}

/// A bare map field: count, then sorted `(key, value)` entries — the
/// representation is a `BTreeMap`, so the order is a property of the type.
pub(crate) fn map_value<H: Hasher>(state: &mut H, map: &BTreeMap<String, Value>) {
    count(state, map.len());
    for (key, value) in map {
        str_value(state, key);
        value.digest(state);
    }
}

/// Absent-before-present over bare map fields.
pub(crate) fn cmp_option_map(
    left: Option<&BTreeMap<String, Value>>,
    right: Option<&BTreeMap<String, Value>>,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => cmp_map(left, right),
    }
}

/// Absent-before-present, then the values.
pub(crate) fn cmp_option<T: Canonical>(left: Option<&T>, right: Option<&T>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => left.canonical_cmp(right),
    }
}

/// Element-wise, shorter-prefix-first: slice order under the canonical
/// element order.
pub(crate) fn cmp_slice<T: Canonical>(left: &[T], right: &[T]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| left.canonical_cmp(right))
        .find(|&ordering| ordering != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

/// Entry-wise over two sorted maps.
pub(crate) fn cmp_map(left: &BTreeMap<String, Value>, right: &BTreeMap<String, Value>) -> Ordering {
    left.iter()
        .zip(right)
        .map(|((left_key, left_value), (right_key, right_value))| {
            left_key
                .cmp(right_key)
                .then_with(|| left_value.canonical_cmp(right_value))
        })
        .find(|&ordering| ordering != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

impl Canonical for String {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Canonical for bool {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Canonical for u64 {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}
