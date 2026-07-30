// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Typed access to the annotation vocabulary.
//!
//! Extensions are resolved by name against the descriptor pool rather than by
//! hardcoded number, and they are resolved once, up front, together with the
//! shape of each annotation message. That ordering is the point: an option the
//! pool does not define, does not declare a field the compiler reads, or
//! declares it with an unreadable type is an error at
//! [`Vocabulary::resolve`]. Afterwards a `None` means "this declaration
//! carries no annotation" and nothing else.
//!
//! Reading lazily instead would make a renamed option, a dropped import, a
//! renumbered extension, or an integer widened from `uint32` to `uint64` — all
//! wire-compatible edits — indistinguishable from an unannotated field. The
//! invariants would read as their defaults and quietly switch themselves off,
//! which is the one failure mode the compiler must not have.

use std::fmt;

use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::ExtensionDescriptor;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;
use prost_reflect::OneofDescriptor;
use prost_reflect::ReflectMessage as _;

/// Full name of the field-level annotation.
pub const FIELD_INVARIANT: &str = "nv.telemetry.options.v1.field_invariant";

/// Full name of the message-level annotation.
pub const MESSAGE_INVARIANT: &str = "nv.telemetry.options.v1.message_invariant";

/// Full name of the oneof-level annotation.
pub const ONEOF_INVARIANT: &str = "nv.telemetry.options.v1.oneof_invariant";

/// Message the compiler round-trips to prove annotations survived encoding.
pub const CANARY: &str = "nv.telemetry.options.v1.Canary";

/// The annotation vocabulary does not have the shape the compiler reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VocabularyError {
    name: String,
    kind: VocabularyErrorKind,
}

/// What is wrong with an annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VocabularyErrorKind {
    /// The schema does not define the extension at all.
    NotDefined,
    /// The extension exists but does not declare a field the compiler reads.
    MissingField {
        /// Name of the absent field.
        field: &'static str,
    },
    /// The field exists with a type the compiler cannot read, which would
    /// otherwise return the type's default and switch the invariant off.
    WrongType {
        /// Name of the field.
        field: &'static str,
        /// Type the compiler expects.
        expected: &'static str,
    },
    /// The canary's annotations did not read back as declared, so option
    /// values were lost somewhere on the encoding path.
    CanaryFailed {
        /// What the canary expected to observe.
        expectation: &'static str,
    },
}

impl VocabularyError {
    /// Full name of the annotation at fault.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What is wrong with it.
    pub fn kind(&self) -> VocabularyErrorKind {
        self.kind
    }
}

impl fmt::Display for VocabularyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            VocabularyErrorKind::NotDefined => write!(
                f,
                "annotation `{}` is not defined by the schema; every invariant \
                 it carries would silently read as unset",
                self.name
            ),
            VocabularyErrorKind::MissingField { field } => write!(
                f,
                "annotation `{}` does not declare `{field}`; the invariant it \
                 carries would silently read as unset",
                self.name
            ),
            VocabularyErrorKind::WrongType { field, expected } => write!(
                f,
                "annotation `{}` declares `{field}` with a type this compiler \
                 cannot read; expected {expected}, and reading it would return \
                 a default that silently switches the invariant off",
                self.name
            ),
            VocabularyErrorKind::CanaryFailed { expectation } => write!(
                f,
                "annotation self-test failed on `{}`: {expectation}. Either \
                 option values were dropped on the encoding path, or the \
                 canary and the compiler disagree about what the vocabulary \
                 means; in both cases invariants would read as unset",
                self.name
            ),
        }
    }
}

impl std::error::Error for VocabularyError {}

/// Scalar types the compiler reads out of an annotation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Bool,
    Uint32,
}

impl Scalar {
    fn accepts(self, kind: &Kind) -> bool {
        matches!(
            (self, kind),
            (Self::Bool, Kind::Bool) | (Self::Uint32, Kind::Uint32)
        )
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Uint32 => "uint32",
        }
    }
}

const FIELD_SHAPE: &[(&str, Scalar)] = &[
    ("zero_is_meaningful", Scalar::Bool),
    ("finite", Scalar::Bool),
    ("required", Scalar::Bool),
    ("unordered", Scalar::Bool),
    ("max_items", Scalar::Uint32),
    ("max_len", Scalar::Uint32),
];

const MESSAGE_SHAPE: &[(&str, Scalar)] = &[
    ("validated", Scalar::Bool),
    ("hashable", Scalar::Bool),
    ("max_depth", Scalar::Uint32),
];

const ONEOF_SHAPE: &[(&str, Scalar)] = &[("required", Scalar::Bool)];

/// Constraints declared on a single field.
///
/// Bounds are optional rather than zero-defaulted, because `max_items: 0` is a
/// legitimate bound and collapsing it into "unset" would reproduce, inside the
/// compiler, the presence bug the compiler exists to prevent.
// The booleans mirror the annotation message one for one. Collapsing them into
// flags would put the compiler's model out of step with the schema it reads,
// which is the drift every other rule here exists to prevent.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldInvariant {
    /// The zero value is meaningful, exempting the field from the presence
    /// lint.
    pub zero_is_meaningful: bool,
    /// Doubles must be finite.
    pub finite: bool,
    /// Validators reject a message in which this field is absent.
    pub required: bool,
    /// The order of this repeated field is not semantic, so it is sorted.
    pub unordered: bool,
    /// Upper bound on the length of a repeated field.
    pub max_items: Option<u32>,
    /// Upper bound on a string or bytes length.
    pub max_len: Option<u32>,
}

/// Constraints declared on a message as a whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MessageInvariant {
    /// Emit a validated wrapper.
    pub validated: bool,
    /// Emit logical content hashing.
    pub hashable: bool,
    /// Recursion bound for a self-referential message.
    pub max_depth: Option<u32>,
}

/// Constraints declared on a oneof.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OneofInvariant {
    /// Validators reject a message in which no case is set.
    pub required: bool,
}

/// The annotation vocabulary, resolved against one pool.
#[derive(Clone, Debug)]
pub struct Vocabulary {
    field: ExtensionDescriptor,
    message: ExtensionDescriptor,
    oneof: ExtensionDescriptor,
}

impl Vocabulary {
    /// Resolves every annotation the compiler reads, checks that each declares
    /// the fields it is about to read with types it can read, and round-trips
    /// the canary to prove option values survived encoding.
    ///
    /// # Errors
    ///
    /// Returns [`VocabularyError`] describing the first annotation that is
    /// absent, incomplete, unreadable as declared, or whose values were lost.
    pub fn resolve(pool: &DescriptorPool) -> Result<Self, VocabularyError> {
        let vocabulary = Self {
            field: extension(pool, FIELD_INVARIANT, FIELD_SHAPE)?,
            message: extension(pool, MESSAGE_INVARIANT, MESSAGE_SHAPE)?,
            oneof: extension(pool, ONEOF_INVARIANT, ONEOF_SHAPE)?,
        };
        vocabulary.check_canary(pool)?;
        Ok(vocabulary)
    }

    /// Reads the invariant annotation on `field`, if it carries one.
    pub fn field_invariant(&self, field: &FieldDescriptor) -> Option<FieldInvariant> {
        let annotation = read(&self.field, &field.options())?;
        Some(FieldInvariant {
            zero_is_meaningful: boolean(&annotation, "zero_is_meaningful"),
            finite: boolean(&annotation, "finite"),
            required: boolean(&annotation, "required"),
            unordered: boolean(&annotation, "unordered"),
            max_items: unsigned(&annotation, "max_items"),
            max_len: unsigned(&annotation, "max_len"),
        })
    }

    /// Reads the invariant annotation on `message`, if it carries one.
    pub fn message_invariant(&self, message: &MessageDescriptor) -> Option<MessageInvariant> {
        let annotation = read(&self.message, &message.options())?;
        Some(MessageInvariant {
            validated: boolean(&annotation, "validated"),
            hashable: boolean(&annotation, "hashable"),
            max_depth: unsigned(&annotation, "max_depth"),
        })
    }

    /// Reads the invariant annotation on `oneof`, if it carries one.
    pub fn oneof_invariant(&self, oneof: &OneofDescriptor) -> Option<OneofInvariant> {
        let annotation = read(&self.oneof, &oneof.options())?;
        Some(OneofInvariant {
            required: boolean(&annotation, "required"),
        })
    }

    fn check_canary(&self, pool: &DescriptorPool) -> Result<(), VocabularyError> {
        let fault = |expectation| VocabularyError {
            name: CANARY.to_owned(),
            kind: VocabularyErrorKind::CanaryFailed { expectation },
        };

        let canary = pool
            .get_message_by_name(CANARY)
            .ok_or_else(|| fault("the canary message is missing from the schema"))?;

        let message = self
            .message_invariant(&canary)
            .ok_or_else(|| fault("the canary carries no message annotation"))?;
        if !message.validated || !message.hashable || message.max_depth != Some(8) {
            return Err(fault("the canary's message annotation read back changed"));
        }

        let reading = canary
            .get_field_by_name("reading")
            .ok_or_else(|| fault("the canary has no `reading` field"))?;
        let reading = self
            .field_invariant(&reading)
            .ok_or_else(|| fault("the canary's `reading` carries no annotation"))?;
        if !reading.finite || !reading.required {
            return Err(fault("the canary's boolean invariants read back false"));
        }

        let labels = canary
            .get_field_by_name("labels")
            .ok_or_else(|| fault("the canary has no `labels` field"))?;
        let labels = self
            .field_invariant(&labels)
            .ok_or_else(|| fault("the canary's `labels` carries no annotation"))?;
        if labels.max_items != Some(4) || labels.max_len != Some(32) {
            return Err(fault("the canary's numeric bounds read back changed"));
        }

        let revision = canary
            .get_field_by_name("revision")
            .ok_or_else(|| fault("the canary has no `revision` field"))?;
        let revision = self
            .field_invariant(&revision)
            .ok_or_else(|| fault("the canary's `revision` carries no annotation"))?;
        if !revision.zero_is_meaningful {
            return Err(fault("the canary's zero_is_meaningful read back false"));
        }

        Ok(())
    }
}

fn extension(
    pool: &DescriptorPool,
    name: &str,
    shape: &'static [(&'static str, Scalar)],
) -> Result<ExtensionDescriptor, VocabularyError> {
    let fault = |kind| VocabularyError {
        name: name.to_owned(),
        kind,
    };

    let extension = pool
        .get_extension_by_name(name)
        .ok_or_else(|| fault(VocabularyErrorKind::NotDefined))?;

    let Kind::Message(annotation) = extension.kind() else {
        return Err(fault(VocabularyErrorKind::NotDefined));
    };

    for &(field, scalar) in shape {
        let declared = annotation
            .get_field_by_name(field)
            .ok_or_else(|| fault(VocabularyErrorKind::MissingField { field }))?;

        if !scalar.accepts(&declared.kind()) {
            return Err(fault(VocabularyErrorKind::WrongType {
                field,
                expected: scalar.name(),
            }));
        }
    }

    Ok(extension)
}

fn read(extension: &ExtensionDescriptor, options: &DynamicMessage) -> Option<DynamicMessage> {
    if !options.has_extension(extension) {
        return None;
    }
    options.get_extension(extension).as_message().cloned()
}

fn boolean(annotation: &DynamicMessage, field: &str) -> bool {
    annotation
        .get_field_by_name(field)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn unsigned(annotation: &DynamicMessage, field: &str) -> Option<u32> {
    let descriptor = annotation.descriptor().get_field_by_name(field)?;
    if !annotation.has_field(&descriptor) {
        return None;
    }
    annotation.get_field(&descriptor).as_u32()
}
