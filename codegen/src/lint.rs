// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Schema rules the compiler enforces before it generates anything.
//!
//! Every rule here exists to keep one property true: nothing in the contract
//! can represent a value the device never reported. proto3 makes that easy to
//! lose, in more ways than the obvious one, so the rules cover the indirect
//! routes as well — a scalar hidden inside a foreign message, a map value, a
//! proto2 `required` field that prost lowers to a bare scalar.

use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt;

use prost_reflect::DescriptorPool;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;
use prost_reflect::Syntax;

use crate::is_contract_package;
use crate::options::FieldInvariant;
use crate::options::Vocabulary;

/// Why the schema was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A scalar with proto3 implicit presence, where an unset value and a
    /// zero value are the same bytes.
    ImplicitPresence,
    /// A map whose values are scalars, so an entry that carries only its key
    /// decodes to a zero value.
    ScalarMapValue,
    /// A field whose message type is declared outside the contract, so the
    /// scalars inside it are never checked.
    ForeignMessageType(String),
    /// A contract file that is not proto3.
    UnsupportedSyntax,
    /// An extension declared in the contract.
    ContractExtension,
    /// A message asking to be hashed without asking to be validated.
    HashableWithoutValidated,
    /// An annotation that cannot apply to the declaration carrying it.
    NotApplicable {
        /// The annotation at fault.
        option: &'static str,
        /// What it does apply to.
        applies_to: &'static str,
    },
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImplicitPresence => f.write_str(
                "implicit-presence scalar: declare `optional`, or annotate \
                 zero_is_meaningful if its zero value is real data",
            ),
            Self::ScalarMapValue => f.write_str(
                "map with a scalar value type: an entry carrying only its key \
                 decodes to a zero value, so use a message value type",
            ),
            Self::ForeignMessageType(name) => write!(
                f,
                "field type `{name}` is declared outside the contract, so the \
                 invariant rules never see its scalars and an empty one decodes \
                 to their zero values. The google.protobuf wrapper types are \
                 the common case; declare the type in the contract instead",
            ),
            Self::UnsupportedSyntax => f.write_str(
                "contract files must be proto3: a proto2 `required` scalar \
                 reports as having presence but is generated as a bare value \
                 with no way to express absence",
            ),
            Self::ContractExtension => f.write_str(
                "the contract must not declare extensions; they are a \
                 declaration site the invariant rules do not reach",
            ),
            Self::HashableWithoutValidated => f.write_str(
                "`hashable` requires `validated`: equal content only hashes \
                 equal once canonicalization has run, and canonicalization \
                 runs in validated construction",
            ),
            Self::NotApplicable { option, applies_to } => write!(
                f,
                "`{option}` does not apply here; it applies to {applies_to}, \
                 and as written it would silently do nothing",
            ),
        }
    }
}

/// One declaration that breaks a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    subject: String,
    reason: Reason,
}

impl Violation {
    /// Name of the offending declaration: a field, a message, an extension, or
    /// a file, depending on the rule.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Which rule it breaks.
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.reason)
    }
}

/// Checks every rule over the contract package.
///
/// The headline rule is presence. proto3 encodes an unset scalar as its zero
/// value, so a reading of 0.0 and a reading that was never taken are the same
/// bytes. Every scalar the contract can reach therefore either declares
/// explicit presence or states that its zero value is real data. This is the
/// schema-level form of the rule that a failed request emits no fabricated
/// observation.
pub fn presence(pool: &DescriptorPool, vocabulary: &Vocabulary) -> Vec<Violation> {
    let mut violations = Vec::new();

    for file in pool.files() {
        if is_contract_package(file.package_name()) && file.syntax() != Syntax::Proto3 {
            violations.push(Violation {
                subject: file.name().to_owned(),
                reason: Reason::UnsupportedSyntax,
            });
        }
    }

    for extension in pool.all_extensions() {
        if is_contract_package(extension.parent_file().package_name()) {
            violations.push(Violation {
                subject: extension.full_name().to_owned(),
                reason: Reason::ContractExtension,
            });
        }
    }

    let hash_reachable = hash_reachable(pool, vocabulary);

    for message in pool.all_messages() {
        if !is_contract_package(message.package_name()) || message.is_map_entry() {
            continue;
        }

        let hash_visible = hash_reachable.contains(message.full_name());
        check_message(&message, vocabulary, hash_visible, &mut violations);
    }

    violations.sort_by(|left, right| left.subject.cmp(&right.subject));
    violations
}

/// Every message a hashable message can reach through fields, including map
/// entries. `collection_metadata` outside this set is a no-op: nothing ever
/// hashes the field, so nothing ever skips it.
fn hash_reachable(pool: &DescriptorPool, vocabulary: &Vocabulary) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut queue: VecDeque<MessageDescriptor> = pool
        .all_messages()
        .filter(|message| {
            vocabulary
                .message_invariant(message)
                .is_some_and(|invariant| invariant.hashable)
        })
        .collect();

    while let Some(message) = queue.pop_front() {
        if !reachable.insert(message.full_name().to_owned()) {
            continue;
        }
        for field in message.fields() {
            if let Kind::Message(nested) = field.kind() {
                queue.push_back(nested);
            }
        }
    }

    reachable
}

fn check_message(
    message: &MessageDescriptor,
    vocabulary: &Vocabulary,
    hash_visible: bool,
    violations: &mut Vec<Violation>,
) {
    if let Some(invariant) = vocabulary.message_invariant(message) {
        if invariant.hashable && !invariant.validated {
            violations.push(Violation {
                subject: message.full_name().to_owned(),
                reason: Reason::HashableWithoutValidated,
            });
        }
    }

    for field in message.fields() {
        let invariant = vocabulary.field_invariant(&field);
        if let Some(invariant) = invariant {
            check_applicability(&field, invariant, hash_visible, violations);
        }

        // A map entry is an ordinary two-field message on the wire, and both
        // its fields have implicit presence. An entry that carries only its
        // key therefore decodes to a zero value. The entry is synthesized and
        // cannot be annotated, so the rule lands on the map field: require a
        // message value type, where an absent value stays absent.
        if field.is_map() {
            // The value type gets both checks a plain field would get: a
            // scalar fabricates a zero when the entry omits it, and a foreign
            // message fabricates one inside itself.
            match map_value_kind(&field) {
                Some(Kind::Message(nested)) if !is_contract_package(nested.package_name()) => {
                    violations.push(Violation {
                        subject: field.full_name().to_owned(),
                        reason: Reason::ForeignMessageType(nested.full_name().to_owned()),
                    });
                }
                Some(Kind::Message(_)) => {}
                _ => violations.push(Violation {
                    subject: field.full_name().to_owned(),
                    reason: Reason::ScalarMapValue,
                }),
            }
            continue;
        }

        // A message-typed field carries presence, but that says nothing about
        // the scalars inside it. Those are only checked if the message is part
        // of the contract; google.protobuf.DoubleValue is the trap, since an
        // empty one reads back as Some(0.0).
        if let Kind::Message(nested) = field.kind() {
            if !is_contract_package(nested.package_name()) {
                violations.push(Violation {
                    subject: field.full_name().to_owned(),
                    reason: Reason::ForeignMessageType(nested.full_name().to_owned()),
                });
            }
            continue;
        }

        // A repeated field distinguishes empty from absent by its length.
        if field.is_list() {
            continue;
        }

        if field.supports_presence() {
            continue;
        }
        if invariant.is_some_and(|it| it.zero_is_meaningful) {
            continue;
        }

        violations.push(Violation {
            subject: field.full_name().to_owned(),
            reason: Reason::ImplicitPresence,
        });
    }
}

fn check_applicability(
    field: &FieldDescriptor,
    invariant: FieldInvariant,
    hash_visible: bool,
    violations: &mut Vec<Violation>,
) {
    let mut reject = |option, applies_to| {
        violations.push(Violation {
            subject: field.full_name().to_owned(),
            reason: Reason::NotApplicable { option, applies_to },
        });
    };

    if invariant.finite && !matches!(field.kind(), Kind::Double | Kind::Float) {
        reject("finite", "double and float fields");
    }
    if invariant.max_items.is_some() && !field.is_list() && !field.is_map() {
        reject("max_items", "repeated and map fields");
    }
    // A protobuf map has no wire order, so there is nothing for
    // canonicalization to sort. Permitting it here would be the silent no-op
    // this rule exists to catch.
    if invariant.unordered && !field.is_list() {
        reject("unordered", "repeated fields");
    }
    if invariant.max_len.is_some() && !matches!(field.kind(), Kind::String | Kind::Bytes) {
        reject("max_len", "string and bytes fields");
    }
    if invariant.zero_is_meaningful
        && (field.supports_presence() || field.is_list() || field.is_map())
    {
        reject(
            "zero_is_meaningful",
            "scalars that have no presence of their own",
        );
    }
    // A field whose zero value is real data is never absent, so requiring its
    // presence either says nothing or contradicts the other annotation.
    // Neither reading is one the author meant.
    if invariant.zero_is_meaningful && invariant.required {
        reject(
            "required",
            "fields that can be absent, which a zero_is_meaningful field cannot",
        );
    }
    // A zero_is_meaningful field is hashed unconditionally — there is no
    // "present" to test — so excluding it from the hash contradicts the other
    // annotation, and whichever generator ran first would silently win.
    if invariant.zero_is_meaningful && invariant.collection_metadata {
        reject(
            "collection_metadata",
            "fields hashing can skip, which a zero_is_meaningful field is not",
        );
    }
    if invariant.collection_metadata && !hash_visible {
        reject(
            "collection_metadata",
            "fields of messages a hashable message reaches; nothing hashes \
             this one, so there is nothing to skip",
        );
    }
}

fn map_value_kind(field: &FieldDescriptor) -> Option<Kind> {
    let Kind::Message(entry) = field.kind() else {
        return None;
    };
    Some(entry.map_entry_value_field().kind())
}
