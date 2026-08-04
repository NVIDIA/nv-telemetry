// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Direct wire encoding from validated types.
//!
//! The naive encode path clones the whole batch to rebuild the wire tree and
//! hand it to prost — measured at roughly half the cost of encoding. This is
//! the other half of the bargain the owned model struck: conversion consumes
//! the wire tree on the way in, so going out either rebuilds it or walks the
//! validated representation directly. [`Emit`] is that walk — the same
//! field-number-ordered traversal the digest does, emitting protobuf wire
//! bytes through `prost::encoding` primitives instead of hash input, and
//! including collection metadata, because the wire is fidelity where the hash
//! is content.
//!
//! Two passes, as prost itself encodes: nested messages are length-prefixed,
//! so a message's length is computed before its bytes are written. The
//! output must be byte-identical to prost encoding the rebuilt wire tree —
//! the tests hold both paths to that, and canonicalization is what makes the
//! claim meaningful: there is exactly one wire form of a validated value.

use std::collections::BTreeMap;

use prost::bytes::BufMut;
use prost::encoding::encode_key;
use prost::encoding::encode_varint;
use prost::encoding::encoded_len_varint;
use prost::encoding::key_len;
use prost::encoding::WireType;

use crate::Value;

/// Emits a message's wire *body* — its fields, unprefixed. The containing
/// field writes the key and length via [`nested`].
pub(crate) trait Emit {
    fn emit(&self, buf: &mut impl BufMut);
    fn emitted_len(&self) -> usize;
}

/// A message-typed field: key, length, body.
pub(crate) fn nested(tag: u32, message: &impl Emit, buf: &mut impl BufMut) {
    let length = message.emitted_len();
    encode_key(tag, WireType::LengthDelimited, buf);
    encode_varint(length as u64, buf);
    message.emit(buf);
}

pub(crate) fn nested_len(tag: u32, message: &impl Emit) -> usize {
    let length = message.emitted_len();
    key_len(tag) + encoded_len_varint(length as u64) + length
}

/// A bare `Value.Map` field: the map message, its entry list, and each
/// entry's key/value pair, hand-rolled because the validated form never
/// materializes the wire's entry structs.
pub(crate) fn map_field(tag: u32, map: &BTreeMap<String, Value>, buf: &mut impl BufMut) {
    let body = map_body_len(map);
    encode_key(tag, WireType::LengthDelimited, buf);
    encode_varint(body as u64, buf);
    for (key, value) in map {
        let entry = entry_len(key, value);
        encode_key(1, WireType::LengthDelimited, buf);
        encode_varint(entry as u64, buf);
        prost::encoding::string::encode(1, key, buf);
        nested(2, value, buf);
    }
}

pub(crate) fn map_field_len(tag: u32, map: &BTreeMap<String, Value>) -> usize {
    let body = map_body_len(map);
    key_len(tag) + encoded_len_varint(body as u64) + body
}

fn map_body_len(map: &BTreeMap<String, Value>) -> usize {
    map.iter()
        .map(|(key, value)| {
            let entry = entry_len(key, value);
            key_len(1) + encoded_len_varint(entry as u64) + entry
        })
        .sum()
}

fn entry_len(key: &String, value: &Value) -> usize {
    prost::encoding::string::encoded_len(1, key) + nested_len(2, value)
}
