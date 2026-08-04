// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The validated value vocabulary: the owned forms of `value.proto`.
//!
//! Hand-written where the rest of the model is generated, because these are
//! the types whose validated shape is nothing like their wire shape: a wire
//! `Value` is a struct holding an optional oneof, and its validated form is a
//! tree; a wire `Map` is an entry list, and its validated form is a sorted
//! map that cannot hold a duplicate key. The bounds enforced here are the
//! generated [`limits`] constants, so a bound is the schema's number and a
//! renamed schema field breaks this file at compile time; the shapes are
//! reviewed like any other hand-written code.
//!
//! The cross-field rules `value.proto` states in prose — nanoseconds within
//! the second, one representation per instant — are enforced inline here
//! rather than through the rules registry, because these types are their own
//! constructors: there is no generated caller to hang a hook on.
//!
//! Enums with invariants on their payloads are opaque. A public enum's arms
//! are always constructible, so `Value` exposing `String(String)` directly
//! would let a caller bypass the length bound; instead construction goes
//! through checked constructors and reading goes through [`Value::kind`],
//! which borrows. [`NumericValue`] stays a plain public enum because its arms
//! already hold validated payloads.

use std::collections::BTreeMap;

use crate::generated::limits;
use crate::generated::wire;
use crate::Finite;
use crate::Invalid;
use crate::Violation;

/// An instant in time, UTC.
///
/// One instant has exactly one representation: nanoseconds are bounded below
/// one second, so equal instants compare and hash equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    seconds: i64,
    nanos: u32,
}

impl Timestamp {
    /// An instant `seconds` from the Unix epoch, plus `nanos` within the
    /// second.
    ///
    /// # Errors
    ///
    /// Refuses `nanos` of one second or more: carrying a second in the
    /// nanosecond field would give one instant two representations.
    pub fn new(seconds: i64, nanos: u32) -> Result<Self, Invalid> {
        // A prose rule from the schema, not an annotation: the vocabulary has
        // no numeric range.
        if nanos >= 1_000_000_000 {
            return Err(Invalid::field(
                "nanos",
                Violation::Rule("nanoseconds must fall within the second"),
            ));
        }
        Ok(Self { seconds, nanos })
    }

    /// Seconds since the Unix epoch.
    #[must_use]
    pub fn seconds(self) -> i64 {
        self.seconds
    }

    /// Nanoseconds within the second, always below one second.
    #[must_use]
    pub fn nanos(self) -> u32 {
        self.nanos
    }
}

impl TryFrom<wire::Timestamp> for Timestamp {
    type Error = Invalid;

    fn try_from(wire: wire::Timestamp) -> Result<Self, Invalid> {
        let seconds = wire
            .seconds
            .ok_or_else(|| Invalid::field("seconds", Violation::Absent))?;
        let nanos = wire
            .nanos
            .ok_or_else(|| Invalid::field("nanos", Violation::Absent))?;
        Self::new(seconds, nanos)
    }
}

impl From<Timestamp> for wire::Timestamp {
    fn from(timestamp: Timestamp) -> Self {
        Self {
            seconds: Some(timestamp.seconds),
            nanos: Some(timestamp.nanos),
        }
    }
}

/// A numeric sample: one of the three arms the schema declares, chosen by the
/// source field's declared type rather than by the value observed.
///
/// A plain public enum, unlike [`Value`]: every arm's payload already upholds
/// its own invariant — [`Finite`] cannot hold a `NaN` — so constructing an
/// arm directly cannot construct an invalid sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NumericValue {
    /// A finite double.
    Double(Finite),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer — the arm a 64-bit counter takes, which a double
    /// would silently truncate above 2^53.
    Uint(u64),
}

impl NumericValue {
    /// A double sample.
    ///
    /// # Errors
    ///
    /// Refuses `NaN` and the infinities: a non-finite reading is a fabricated
    /// observation, reported as an invalid field rather than carried.
    pub fn double(value: f64) -> Result<Self, Invalid> {
        Finite::new(value)
            .map(Self::Double)
            .ok_or_else(|| Invalid::field("double_value", Violation::NotFinite))
    }
}

impl TryFrom<wire::NumericValue> for NumericValue {
    type Error = Invalid;

    fn try_from(wire: wire::NumericValue) -> Result<Self, Invalid> {
        let kind = wire
            .kind
            .ok_or_else(|| Invalid::field("kind", Violation::Absent))?;
        match kind {
            wire::numeric_value::Kind::DoubleValue(value) => Self::double(value),
            wire::numeric_value::Kind::IntValue(value) => Ok(Self::Int(value)),
            wire::numeric_value::Kind::UintValue(value) => Ok(Self::Uint(value)),
        }
    }
}

impl From<NumericValue> for wire::NumericValue {
    fn from(value: NumericValue) -> Self {
        let kind = match value {
            NumericValue::Double(value) => wire::numeric_value::Kind::DoubleValue(value.get()),
            NumericValue::Int(value) => wire::numeric_value::Kind::IntValue(value),
            NumericValue::Uint(value) => wire::numeric_value::Kind::UintValue(value),
        };
        Self { kind: Some(kind) }
    }
}

/// A recursive property value: null, bool, integers, finite double, string,
/// bytes, timestamp, list, or key-sorted map.
///
/// Opaque, because several arms carry their own bounds and a public enum's
/// arms are always constructible. Construction goes through the checked
/// constructors; reading goes through [`Value::kind`]. Maps are sorted and
/// duplicate-free by representation — a `BTreeMap` — so the canonical entry
/// order the schema asks for is a property of the type rather than a pass
/// over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    repr: Repr,
    /// Logical nesting depth: a scalar is 1, a container is one more than its
    /// deepest child. Computed at construction so the bound is checked once,
    /// not re-walked.
    depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Repr {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Double(Finite),
    String(String),
    Bytes(Vec<u8>),
    Timestamp(Timestamp),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

/// A borrowed view of a [`Value`] for pattern matching.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ValueKind<'a> {
    /// Present and explicitly null — a different fact from absent.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    Uint(u64),
    /// A finite double.
    Double(Finite),
    /// A string.
    String(&'a str),
    /// Bytes.
    Bytes(&'a [u8]),
    /// An instant.
    Timestamp(Timestamp),
    /// A list whose order is data.
    List(&'a [Value]),
    /// A key-sorted map.
    Map(&'a BTreeMap<String, Value>),
}

impl Value {
    /// The explicit null.
    #[must_use]
    pub fn null() -> Self {
        Self {
            repr: Repr::Null,
            depth: 1,
        }
    }

    /// A boolean value.
    #[must_use]
    pub fn bool(value: bool) -> Self {
        Self {
            repr: Repr::Bool(value),
            depth: 1,
        }
    }

    /// A signed integer value.
    #[must_use]
    pub fn int(value: i64) -> Self {
        Self {
            repr: Repr::Int(value),
            depth: 1,
        }
    }

    /// An unsigned integer value.
    #[must_use]
    pub fn uint(value: u64) -> Self {
        Self {
            repr: Repr::Uint(value),
            depth: 1,
        }
    }

    /// A double value.
    ///
    /// # Errors
    ///
    /// Refuses `NaN` and the infinities.
    pub fn double(value: f64) -> Result<Self, Invalid> {
        Finite::new(value)
            .map(|value| Self {
                repr: Repr::Double(value),
                depth: 1,
            })
            .ok_or_else(|| Invalid::field("double_value", Violation::NotFinite))
    }

    /// A string value.
    ///
    /// # Errors
    ///
    /// Refuses a string over the schema's byte bound.
    pub fn string(value: impl Into<String>) -> Result<Self, Invalid> {
        let value = value.into();
        check_len(
            value.len(),
            limits::VALUE_STRING_VALUE_MAX_LEN,
            "string_value",
        )?;
        Ok(Self {
            repr: Repr::String(value),
            depth: 1,
        })
    }

    /// A bytes value.
    ///
    /// # Errors
    ///
    /// Refuses bytes over the schema's bound.
    pub fn bytes(value: impl Into<Vec<u8>>) -> Result<Self, Invalid> {
        let value = value.into();
        check_len(
            value.len(),
            limits::VALUE_BYTES_VALUE_MAX_LEN,
            "bytes_value",
        )?;
        Ok(Self {
            repr: Repr::Bytes(value),
            depth: 1,
        })
    }

    /// A timestamp value.
    #[must_use]
    pub fn timestamp(value: Timestamp) -> Self {
        Self {
            repr: Repr::Timestamp(value),
            depth: 1,
        }
    }

    /// A list value. Order is data: it is preserved exactly, never sorted.
    ///
    /// # Errors
    ///
    /// Refuses a list over the schema's element bound, or one nested past the
    /// schema's depth bound.
    pub fn list(values: Vec<Value>) -> Result<Self, Invalid> {
        check_count(values.len(), limits::VALUE_LIST_VALUES_MAX_ITEMS, "values")?;
        let depth = container_depth(values.iter(), "values")?;
        Ok(Self {
            repr: Repr::List(values),
            depth,
        })
    }

    /// A map value, sorted by key.
    ///
    /// Entries may arrive in any order; the representation sorts them. What
    /// it refuses to do is choose between two values for one key — a map that
    /// names a key twice has no reading, so it is rejected rather than
    /// last-write-wins.
    ///
    /// # Errors
    ///
    /// Refuses an empty or over-long key, a duplicate key, more entries than
    /// the schema's bound, or nesting past the schema's depth bound.
    pub fn map(entries: impl IntoIterator<Item = (String, Value)>) -> Result<Self, Invalid> {
        let map = collect_map(entries)?;
        let depth = container_depth(map.values(), "entries")?;
        Ok(Self {
            repr: Repr::Map(map),
            depth,
        })
    }

    /// The value, for pattern matching.
    #[must_use]
    pub fn kind(&self) -> ValueKind<'_> {
        match &self.repr {
            Repr::Null => ValueKind::Null,
            Repr::Bool(value) => ValueKind::Bool(*value),
            Repr::Int(value) => ValueKind::Int(*value),
            Repr::Uint(value) => ValueKind::Uint(*value),
            Repr::Double(value) => ValueKind::Double(*value),
            Repr::String(value) => ValueKind::String(value),
            Repr::Bytes(value) => ValueKind::Bytes(value),
            Repr::Timestamp(value) => ValueKind::Timestamp(*value),
            Repr::List(values) => ValueKind::List(values),
            Repr::Map(entries) => ValueKind::Map(entries),
        }
    }

    /// Logical nesting depth: a scalar is 1, a container one more than its
    /// deepest child.
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }
}

impl TryFrom<wire::Value> for Value {
    type Error = Invalid;

    fn try_from(wire: wire::Value) -> Result<Self, Invalid> {
        let kind = wire
            .kind
            .ok_or_else(|| Invalid::field("kind", Violation::Absent))?;
        match kind {
            wire::value::Kind::NullValue(wire::Null {}) => Ok(Self::null()),
            wire::value::Kind::BoolValue(value) => Ok(Self::bool(value)),
            wire::value::Kind::IntValue(value) => Ok(Self::int(value)),
            wire::value::Kind::UintValue(value) => Ok(Self::uint(value)),
            wire::value::Kind::DoubleValue(value) => Self::double(value),
            wire::value::Kind::StringValue(value) => Self::string(value),
            wire::value::Kind::BytesValue(value) => Self::bytes(value),
            wire::value::Kind::TimestampValue(value) => {
                let value =
                    Timestamp::try_from(value).map_err(|error| error.at("timestamp_value"))?;
                Ok(Self::timestamp(value))
            }
            wire::value::Kind::ListValue(list) => {
                let mut values = Vec::with_capacity(list.values.len());
                for (index, value) in list.values.into_iter().enumerate() {
                    values.push(
                        Self::try_from(value).map_err(|error| error.at_index("values", index))?,
                    );
                }
                Self::list(values).map_err(|error| error.at("list_value"))
            }
            wire::value::Kind::MapValue(map) => {
                let mut entries = Vec::with_capacity(map.entries.len());
                for (index, entry) in map.entries.into_iter().enumerate() {
                    let key = entry.key.ok_or_else(|| {
                        Invalid::field("key", Violation::Absent)
                            .at_index("entries", index)
                            .at("map_value")
                    })?;
                    let value = entry.value.ok_or_else(|| {
                        Invalid::field("value", Violation::Absent)
                            .at_index("entries", index)
                            .at("map_value")
                    })?;
                    let value = Self::try_from(value).map_err(|error| {
                        error.at("value").at_index("entries", index).at("map_value")
                    })?;
                    entries.push((key, value));
                }
                Self::map(entries).map_err(|error| error.at("map_value"))
            }
        }
    }
}

impl From<Value> for wire::Value {
    fn from(value: Value) -> Self {
        let kind = match value.repr {
            Repr::Null => wire::value::Kind::NullValue(wire::Null {}),
            Repr::Bool(value) => wire::value::Kind::BoolValue(value),
            Repr::Int(value) => wire::value::Kind::IntValue(value),
            Repr::Uint(value) => wire::value::Kind::UintValue(value),
            Repr::Double(value) => wire::value::Kind::DoubleValue(value.get()),
            Repr::String(value) => wire::value::Kind::StringValue(value),
            Repr::Bytes(value) => wire::value::Kind::BytesValue(value),
            Repr::Timestamp(value) => wire::value::Kind::TimestampValue(value.into()),
            Repr::List(values) => wire::value::Kind::ListValue(wire::value::List {
                values: values.into_iter().map(Self::from).collect(),
            }),
            // BTreeMap iterates sorted, so the wire entries come out in
            // canonical order without a separate pass.
            Repr::Map(entries) => wire::value::Kind::MapValue(wire::value::Map {
                entries: entries
                    .into_iter()
                    .map(|(key, value)| wire::value::map::Entry {
                        key: Some(key),
                        value: Some(value.into()),
                    })
                    .collect(),
            }),
        };
        Self { kind: Some(kind) }
    }
}

/// Depth of a container holding `children`, checked against the schema bound.
///
/// Children were themselves constructed through checked constructors, so each
/// is within the bound; the container is one deeper than the deepest, and an
/// empty container is a scalar's depth.
fn container_depth<'a>(
    children: impl Iterator<Item = &'a Value>,
    field: &str,
) -> Result<u32, Invalid> {
    let deepest = children.map(Value::depth).max().unwrap_or(0);
    let depth = deepest + 1;
    if depth > limits::VALUE_MAX_DEPTH {
        return Err(Invalid::field(
            field,
            Violation::TooDeep {
                limit: limits::VALUE_MAX_DEPTH,
            },
        ));
    }
    Ok(depth)
}

/// Converts a wire map into the validated representation, with the same
/// checks [`Value::map`] runs.
///
/// For the fields typed `Value.Map` directly — attribute sets and observed
/// properties — which hold the map without a `Value` around it.
pub(crate) fn map_from_wire(map: wire::value::Map) -> Result<BTreeMap<String, Value>, Invalid> {
    let mut entries = Vec::with_capacity(map.entries.len());
    for (index, entry) in map.entries.into_iter().enumerate() {
        let key = entry
            .key
            .ok_or_else(|| Invalid::field(&format!("entries[{index}].key"), Violation::Absent))?;
        let value = entry
            .value
            .ok_or_else(|| Invalid::field(&format!("entries[{index}].value"), Violation::Absent))?;
        let value =
            Value::try_from(value).map_err(|error| error.at(&format!("entries[{index}].value")))?;
        entries.push((key, value));
    }
    collect_map(entries)
}

/// Collects entries into the sorted, duplicate-free representation, checking
/// each key's bounds and the entry count — everything a map must be except
/// the depth rule, which only applies under a `Value`.
fn collect_map(
    entries: impl IntoIterator<Item = (String, Value)>,
) -> Result<BTreeMap<String, Value>, Invalid> {
    let mut map = BTreeMap::new();
    for (index, (key, value)) in entries.into_iter().enumerate() {
        if key.is_empty() {
            return Err(Invalid::field("key", Violation::Empty).at_index("entries", index));
        }
        check_len(key.len(), limits::VALUE_MAP_ENTRY_KEY_MAX_LEN, "key")
            .map_err(|error| error.at_index("entries", index))?;
        if map.insert(key, value).is_some() {
            return Err(Invalid::field("key", Violation::Duplicate).at_index("entries", index));
        }
    }
    check_count(map.len(), limits::VALUE_MAP_ENTRIES_MAX_ITEMS, "entries")?;
    Ok(map)
}

/// Rebuilds the wire form of a bare map, entries in canonical (sorted) order.
pub(crate) fn map_into_wire(map: BTreeMap<String, Value>) -> wire::value::Map {
    wire::value::Map {
        entries: map
            .into_iter()
            .map(|(key, value)| wire::value::map::Entry {
                key: Some(key),
                value: Some(value.into()),
            })
            .collect(),
    }
}

/// Checks a bare map a builder was handed directly, where the entries did not
/// pass through [`map_from_wire`]: the representation already guarantees
/// sorted, duplicate-free keys, so what is left is each key's own bounds and
/// the entry count.
pub(crate) fn check_map(map: &BTreeMap<String, Value>, field: &str) -> Result<(), Invalid> {
    for key in map.keys() {
        if key.is_empty() {
            return Err(Invalid::field(field, Violation::Empty));
        }
        if let Some(violation) =
            crate::invalid::too_long(key.len(), limits::VALUE_MAP_ENTRY_KEY_MAX_LEN)
        {
            return Err(Invalid::field(field, violation));
        }
    }
    if let Some(violation) =
        crate::invalid::too_many(map.len(), limits::VALUE_MAP_ENTRIES_MAX_ITEMS)
    {
        return Err(Invalid::field(field, violation));
    }
    Ok(())
}

/// Whether `actual` exceeds a schema byte bound.
fn check_len(actual: usize, limit: u32, field: &str) -> Result<(), Invalid> {
    match crate::invalid::too_long(actual, limit) {
        Some(violation) => Err(Invalid::field(field, violation)),
        None => Ok(()),
    }
}

/// Whether `actual` exceeds a schema element bound.
fn check_count(actual: usize, limit: u32, field: &str) -> Result<(), Invalid> {
    match crate::invalid::too_many(actual, limit) {
        Some(violation) => Err(Invalid::field(field, violation)),
        None => Ok(()),
    }
}

// --- Canonical order and content digest, per docs in `crate::canonical` ---

impl crate::canonical::Canonical for Timestamp {
    fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Field-number order: seconds (1), nanos (2) — which is also time
        // order, and what the derived `Ord` implements.
        self.cmp(other)
    }
}

impl crate::canonical::Digest for Timestamp {
    fn digest<H: std::hash::Hasher>(&self, state: &mut H) {
        crate::canonical::tag(state, 1);
        crate::canonical::i64_value(state, self.seconds);
        crate::canonical::tag(state, 2);
        crate::canonical::u32_value(state, self.nanos);
        crate::canonical::end(state);
    }
}

impl NumericValue {
    /// The arm's wire field number: the discriminant both ordering and
    /// hashing label the payload with, so an integer 5 and a double 5.0 are
    /// different content — arm selection is fixed by the source's declared
    /// type, and a value must not drift between arms.
    fn arm(&self) -> u32 {
        match self {
            Self::Double(_) => 1,
            Self::Int(_) => 2,
            Self::Uint(_) => 3,
        }
    }
}

impl crate::canonical::Canonical for NumericValue {
    fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Double(left), Self::Double(right)) => left.cmp(right),
            (Self::Int(left), Self::Int(right)) => left.cmp(right),
            (Self::Uint(left), Self::Uint(right)) => left.cmp(right),
            _ => self.arm().cmp(&other.arm()),
        }
    }
}

impl crate::canonical::Digest for NumericValue {
    fn digest<H: std::hash::Hasher>(&self, state: &mut H) {
        crate::canonical::tag(state, self.arm());
        match self {
            Self::Double(value) => crate::canonical::f64_value(state, value.get()),
            Self::Int(value) => crate::canonical::i64_value(state, *value),
            Self::Uint(value) => crate::canonical::u64_value(state, *value),
        }
    }
}

impl Value {
    /// The arm's wire field number, as for [`NumericValue::arm`].
    fn arm(&self) -> u32 {
        match &self.repr {
            Repr::Null => 1,
            Repr::Bool(_) => 2,
            Repr::Int(_) => 3,
            Repr::Uint(_) => 4,
            Repr::Double(_) => 5,
            Repr::String(_) => 6,
            Repr::Bytes(_) => 7,
            Repr::Timestamp(_) => 8,
            Repr::List(_) => 9,
            Repr::Map(_) => 10,
        }
    }
}

impl crate::canonical::Canonical for Value {
    fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (&self.repr, &other.repr) {
            (Repr::Null, Repr::Null) => std::cmp::Ordering::Equal,
            (Repr::Bool(left), Repr::Bool(right)) => left.cmp(right),
            (Repr::Int(left), Repr::Int(right)) => left.cmp(right),
            (Repr::Uint(left), Repr::Uint(right)) => left.cmp(right),
            (Repr::Double(left), Repr::Double(right)) => left.cmp(right),
            (Repr::String(left), Repr::String(right)) => left.cmp(right),
            (Repr::Bytes(left), Repr::Bytes(right)) => left.cmp(right),
            (Repr::Timestamp(left), Repr::Timestamp(right)) => left.canonical_cmp(right),
            (Repr::List(left), Repr::List(right)) => crate::canonical::cmp_slice(left, right),
            (Repr::Map(left), Repr::Map(right)) => crate::canonical::cmp_map(left, right),
            _ => self.arm().cmp(&other.arm()),
        }
    }
}

impl crate::canonical::Digest for Value {
    fn digest<H: std::hash::Hasher>(&self, state: &mut H) {
        crate::canonical::tag(state, self.arm());
        match &self.repr {
            // The arm alone: null's content is that it is null.
            Repr::Null => {}
            Repr::Bool(value) => crate::canonical::bool_value(state, *value),
            Repr::Int(value) => crate::canonical::i64_value(state, *value),
            Repr::Uint(value) => crate::canonical::u64_value(state, *value),
            Repr::Double(value) => crate::canonical::f64_value(state, value.get()),
            Repr::String(value) => crate::canonical::str_value(state, value),
            Repr::Bytes(value) => crate::canonical::bytes_value(state, value),
            Repr::Timestamp(value) => value.digest(state),
            Repr::List(values) => {
                crate::canonical::count(state, values.len());
                for value in values {
                    value.digest(state);
                }
            }
            Repr::Map(entries) => crate::canonical::map_value(state, entries),
        }
    }
}

// --- Direct wire encoding, per docs in `crate::encode` ---

impl crate::encode::Emit for Timestamp {
    fn emit(&self, buf: &mut impl prost::bytes::BufMut) {
        prost::encoding::int64::encode(1, &self.seconds, buf);
        prost::encoding::uint32::encode(2, &self.nanos, buf);
    }

    fn emitted_len(&self) -> usize {
        prost::encoding::int64::encoded_len(1, &self.seconds)
            + prost::encoding::uint32::encoded_len(2, &self.nanos)
    }
}

impl crate::encode::Emit for NumericValue {
    fn emit(&self, buf: &mut impl prost::bytes::BufMut) {
        match self {
            Self::Double(value) => prost::encoding::double::encode(1, &value.get(), buf),
            Self::Int(value) => prost::encoding::sint64::encode(2, value, buf),
            Self::Uint(value) => prost::encoding::uint64::encode(3, value, buf),
        }
    }

    fn emitted_len(&self) -> usize {
        match self {
            Self::Double(value) => prost::encoding::double::encoded_len(1, &value.get()),
            Self::Int(value) => prost::encoding::sint64::encoded_len(2, value),
            Self::Uint(value) => prost::encoding::uint64::encoded_len(3, value),
        }
    }
}

impl crate::encode::Emit for Value {
    fn emit(&self, buf: &mut impl prost::bytes::BufMut) {
        use prost::encoding::encode_key;
        use prost::encoding::encode_varint;
        use prost::encoding::WireType;
        match &self.repr {
            // An empty nested message: present, and that is all it says.
            Repr::Null => {
                encode_key(1, WireType::LengthDelimited, buf);
                encode_varint(0, buf);
            }
            Repr::Bool(value) => prost::encoding::bool::encode(2, value, buf),
            Repr::Int(value) => prost::encoding::sint64::encode(3, value, buf),
            Repr::Uint(value) => prost::encoding::uint64::encode(4, value, buf),
            Repr::Double(value) => prost::encoding::double::encode(5, &value.get(), buf),
            Repr::String(value) => prost::encoding::string::encode(6, value, buf),
            Repr::Bytes(value) => prost::encoding::bytes::encode(7, value, buf),
            Repr::Timestamp(value) => crate::encode::nested(8, value, buf),
            Repr::List(values) => {
                let body: usize = values
                    .iter()
                    .map(|value| crate::encode::nested_len(1, value))
                    .sum();
                encode_key(9, WireType::LengthDelimited, buf);
                encode_varint(body as u64, buf);
                for value in values {
                    crate::encode::nested(1, value, buf);
                }
            }
            Repr::Map(entries) => crate::encode::map_field(10, entries, buf),
        }
    }

    fn emitted_len(&self) -> usize {
        use prost::encoding::encoded_len_varint;
        use prost::encoding::key_len;
        match &self.repr {
            Repr::Null => key_len(1) + 1,
            Repr::Bool(value) => prost::encoding::bool::encoded_len(2, value),
            Repr::Int(value) => prost::encoding::sint64::encoded_len(3, value),
            Repr::Uint(value) => prost::encoding::uint64::encoded_len(4, value),
            Repr::Double(value) => prost::encoding::double::encoded_len(5, &value.get()),
            Repr::String(value) => prost::encoding::string::encoded_len(6, value),
            Repr::Bytes(value) => prost::encoding::bytes::encoded_len(7, value),
            Repr::Timestamp(value) => crate::encode::nested_len(8, value),
            Repr::List(values) => {
                let body: usize = values
                    .iter()
                    .map(|value| crate::encode::nested_len(1, value))
                    .sum();
                key_len(9) + encoded_len_varint(body as u64) + body
            }
            Repr::Map(entries) => crate::encode::map_field_len(10, entries),
        }
    }
}
