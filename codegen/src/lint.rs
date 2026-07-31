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
use crate::options;
use crate::options::Applies;
use crate::options::FieldInvariant;
use crate::options::FieldOption;
use crate::options::HashStance;
use crate::options::Vocabulary;
use crate::options::ZeroStance;

/// Why the schema was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A scalar with proto3 implicit presence, where an unset value and a
    /// zero value are the same bytes.
    ImplicitPresence,
    /// A map whose values are scalars, so an entry that carries only its key
    /// decodes to a zero value.
    ScalarMapValue,
    /// A field whose message or enum type is declared outside the contract,
    /// so its contents are never checked.
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
    /// Two annotations on one field asserting opposite things. Derived from
    /// the stance columns of the option table rather than enumerated per pair,
    /// so an option added later conflicts with every existing option it should
    /// without anyone writing the rule.
    Conflicting {
        /// The first option, in the vocabulary's declaration order.
        first: &'static str,
        /// The second.
        second: &'static str,
        /// What they disagree about.
        axis: Contradiction,
    },
    /// A `unique_by` key naming a field the element type does not declare.
    UnknownUniqueKey(String),
    /// A `unique_by` key that can be absent, so two elements that both omit it
    /// would compare equal.
    OptionalUniqueKey(String),
    /// A `unique_by` key that is repeated on the element type.
    RepeatedUniqueKey(String),
    /// A `unique_by` key named more than once in one list.
    DuplicateUniqueKey(String),
    /// A `unique_by` key that is `collection_metadata`, so identity would rest
    /// on something the contract does not treat as content.
    MetadataUniqueKey(String),
    /// A message-typed `unique_by` key whose type is not `validated`, so equal
    /// content is not guaranteed to compare equal.
    UnvalidatedUniqueKey(String),
}

/// What two conflicting annotations disagree about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contradiction {
    /// One reads the field's zero value as real data, the other rejects that
    /// same value.
    ZeroValue,
    /// One hashes the field unconditionally, the other excludes it from
    /// hashing.
    Hashing,
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
            Self::Conflicting {
                first,
                second,
                axis: Contradiction::ZeroValue,
            } => write!(
                f,
                "`{first}` and `{second}` contradict: one reads the field's \
                 zero value as real data, the other rejects that same value, \
                 and whichever generator ran first would silently win",
            ),
            Self::Conflicting {
                first,
                second,
                axis: Contradiction::Hashing,
            } => write!(
                f,
                "`{first}` and `{second}` contradict: one hashes the field \
                 unconditionally — with no absent state there is no `present` \
                 to test — and the other excludes it from hashing",
            ),
            Self::UnknownUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}`, which the element type does not \
                 declare; the uniqueness check has nothing to compare and \
                 every element would pass",
            ),
            Self::OptionalUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}`, which the element type may omit; \
                 two elements that both omit it would compare equal and one \
                 would be rejected as a duplicate of the other. A key must be \
                 `required`, or `zero_is_meaningful` and so never absent",
            ),
            Self::RepeatedUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}`, which is repeated on the element \
                 type; identity would then turn on the order of that field's \
                 own elements, which canonicalization does not settle because \
                 it is not the field being sorted",
            ),
            Self::DuplicateUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}` more than once; the repeat narrows \
                 nothing, so part of the annotation reads as enforced while \
                 doing nothing",
            ),
            Self::MetadataUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}`, which is `collection_metadata` and \
                 so not content; two elements identical in everything hashing \
                 compares would be admitted as distinct, which is the \
                 contradiction the key exists to prevent",
            ),
            Self::UnvalidatedUniqueKey(key) => write!(
                f,
                "`unique_by` names `{key}`, whose message type is not \
                 `validated`; equal content only compares equal once \
                 canonicalization has run, so two elements naming the same \
                 thing would be admitted as distinct",
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
    let self_referential = self_referential(pool);

    for message in pool.all_messages() {
        if !is_contract_package(message.package_name()) || message.is_map_entry() {
            continue;
        }

        let hash_visible = hash_reachable.contains(message.full_name());
        let recursive = self_referential.contains(message.full_name());
        check_message(
            &message,
            vocabulary,
            hash_visible,
            recursive,
            &mut violations,
        );
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
            if vocabulary
                .field_invariant(&field)
                .is_some_and(|invariant| invariant.collection_metadata)
            {
                continue;
            }
            if let Kind::Message(nested) = field.kind() {
                queue.push_back(nested);
            }
        }
    }

    reachable
}

/// Every message that can contain itself, directly or through another message.
///
/// `max_depth` bounds recursion, counted in logical levels of the type it is
/// written on. A message that no chain of fields leads back to has no levels to
/// bound, so the annotation there reads as a limit and enforces nothing.
fn self_referential(pool: &DescriptorPool) -> BTreeSet<String> {
    let mut recursive = BTreeSet::new();

    for message in pool.all_messages() {
        let start = message.full_name().to_owned();
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<MessageDescriptor> = nested_messages(&message).collect();

        while let Some(next) = queue.pop_front() {
            if next.full_name() == start {
                recursive.insert(start.clone());
                break;
            }
            if !seen.insert(next.full_name().to_owned()) {
                continue;
            }
            queue.extend(nested_messages(&next));
        }
    }

    recursive
}

/// The message types a message's fields lead to.
///
/// Map fields lead through their synthetic entry, which is a message like any
/// other, so a map whose value is the containing type is found the same way.
fn nested_messages(message: &MessageDescriptor) -> impl Iterator<Item = MessageDescriptor> + '_ {
    message.fields().filter_map(|field| match field.kind() {
        Kind::Message(nested) => Some(nested),
        _ => None,
    })
}

fn check_message(
    message: &MessageDescriptor,
    vocabulary: &Vocabulary,
    hash_visible: bool,
    recursive: bool,
    violations: &mut Vec<Violation>,
) {
    if let Some(invariant) = vocabulary.message_invariant(message) {
        if invariant.hashable && !invariant.validated {
            violations.push(Violation {
                subject: message.full_name().to_owned(),
                reason: Reason::HashableWithoutValidated,
            });
        }
        // The vocabulary already says `max_depth` is "for a self-referential
        // message such as Value"; this is that sentence enforced. A bound on a
        // type that cannot nest inside itself limits nothing, and reads in the
        // schema as a size limit somebody is relying on.
        if invariant.max_depth.is_some() && !recursive {
            violations.push(Violation {
                subject: message.full_name().to_owned(),
                reason: Reason::NotApplicable {
                    option: "max_depth",
                    applies_to: "messages that can contain themselves",
                },
            });
        }
    }

    for field in message.fields() {
        let invariant = vocabulary.field_invariant(&field);
        if let Some(invariant) = &invariant {
            check_applicability(&field, invariant, vocabulary, hash_visible, violations);
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

        // Enums escape the same way, and the generated wire types render a
        // foreign one as a path out of the module — which either fails to
        // compile or starts resolving the day an unrelated dependency lands.
        if let Kind::Enum(nested) = field.kind() {
            if !is_contract_package(nested.package_name()) {
                violations.push(Violation {
                    subject: field.full_name().to_owned(),
                    reason: Reason::ForeignMessageType(nested.full_name().to_owned()),
                });
                continue;
            }
        }

        // A repeated field distinguishes empty from absent by its length.
        if field.is_list() {
            continue;
        }

        if field.supports_presence() {
            continue;
        }
        if invariant.as_ref().is_some_and(|it| it.zero_is_meaningful) {
            continue;
        }

        violations.push(Violation {
            subject: field.full_name().to_owned(),
            reason: Reason::ImplicitPresence,
        });
    }
}

/// Checks an annotated field against the option table.
///
/// Almost everything here is derived from the table's columns rather than
/// stated per option: where an option applies, which pairs contradict, and
/// what `max_items: 0` makes vacuous. The rules that remain hand-written are
/// the ones a per-option column cannot carry, and each says why.
fn check_applicability(
    field: &FieldDescriptor,
    invariant: &FieldInvariant,
    vocabulary: &Vocabulary,
    hash_visible: bool,
    violations: &mut Vec<Violation>,
) {
    let mut push = |reason| {
        violations.push(Violation {
            subject: field.full_name().to_owned(),
            reason,
        });
    };
    let not_applicable = |option, applies_to| Reason::NotApplicable { option, applies_to };

    // The options written on this field, in the vocabulary's declaration
    // order, so diagnostics come out the same way every run.
    let set: Vec<&FieldOption> = options::FIELD_OPTIONS
        .iter()
        .filter(|option| (option.render)(invariant).is_some())
        .collect();

    // Where each option applies is a column, so an option that cannot mean
    // anything on this field is rejected without a rule naming the pair —
    // including combinations no fixture thought of, which is where the
    // hand-written predecessor of this loop kept being found wrong.
    for option in &set {
        if !option.applies.permits(field) {
            push(not_applicable(option.name, option.applies.description()));
        }
    }

    // A oneof already decides which arm carries the value, so `required` on
    // one member either says nothing or says the oneof has a single legal arm.
    // "Some arm must be set" is spelled on the oneof itself. Hand-written
    // because it depends on where the field sits, not on what the field is —
    // an `Applies` column cannot see the containing oneof.
    //
    // This is not tidiness. `unique_by` reads `required` as "an element always
    // carries this", and a oneof member is absent whenever a sibling arm holds
    // the value — so without this rule a key can be absent while claiming it
    // cannot, which is the one thing the key rule exists to prevent.
    if invariant.required && in_declared_oneof(field) {
        push(not_applicable(
            "required",
            "fields outside a oneof; a oneof states that some arm must be set \
             with its own `required`",
        ));
    }

    // Contradictions are derived from the stance columns: any two options
    // taking opposite positions on the zero value or on hash visibility are
    // rejected as a pair. `zero_is_meaningful` against `non_empty`,
    // `reject_unspecified`, or `collection_metadata` all fall out of this one
    // loop, as does the same clash for any option added later.
    for (position, first) in set.iter().enumerate() {
        for second in &set[position + 1..] {
            if let Some(axis) = contradiction(first, second) {
                push(Reason::Conflicting {
                    first: first.name,
                    second: second.name,
                    axis,
                });
            }
        }
    }

    // Hand-written because it is reachability, not shape: whether anything
    // ever hashes this field is a property of which hashable messages can
    // reach its parent, and a column on the option cannot know that.
    if invariant.collection_metadata && !hash_visible {
        push(not_applicable(
            "collection_metadata",
            "fields of messages a hashable message reaches; nothing hashes \
             this one, so there is nothing to skip",
        ));
    }

    // Hand-written because it is value-dependent: `max_len` at any other value
    // coexists with `non_empty` happily, and a stance column has no way to say
    // "invalid only at zero". The empty value cannot be both the only
    // permitted one and forbidden.
    if invariant.non_empty && invariant.max_len == Some(0) {
        push(not_applicable(
            "non_empty",
            "fields that may hold something, which `max_len: 0` forbids",
        ));
    }

    // `max_items: 0` says the field holds no elements, so every option that
    // describes an element — a column — describes nothing. Reported one at a
    // time so the message names the annotation to delete.
    if invariant.max_items == Some(0) {
        for option in &set {
            if option.per_element {
                push(not_applicable(
                    option.name,
                    "fields that can hold an element, which `max_items: 0` \
                     forbids",
                ));
            }
        }
    }

    // Key checks run only where `unique_by` itself is legal; where it is not,
    // the applicability loop above has already said so, and key-level errors
    // on top of that would be noise about an annotation that cannot stay.
    if !invariant.unique_by.is_empty() && Applies::MessageLists.permits(field) {
        check_unique_by(field, invariant, vocabulary, violations);
    }
}

/// The axis on which two options contradict, if they do.
fn contradiction(first: &FieldOption, second: &FieldOption) -> Option<Contradiction> {
    if matches!(
        (first.zero, second.zero),
        (ZeroStance::ZeroIsData, ZeroStance::ZeroIsInvalid)
            | (ZeroStance::ZeroIsInvalid, ZeroStance::ZeroIsData)
    ) {
        return Some(Contradiction::ZeroValue);
    }
    if matches!(
        (first.hash, second.hash),
        (HashStance::AlwaysHashed, HashStance::SkippedByHash)
            | (HashStance::SkippedByHash, HashStance::AlwaysHashed)
    ) {
        return Some(Contradiction::Hashing);
    }
    None
}

/// Checks that `unique_by` can name what it claims to.
///
/// Every failure here is silent at runtime rather than loud: a key that does
/// not resolve compares nothing, so every element passes and the annotation
/// reads as enforced while enforcing nothing.
///
/// A key must be four things at once — resolvable, singular, always present,
/// and content whose equality is sound — and each check below is one of them.
/// The caller has already established the field itself is a repeated message,
/// which is `unique_by`'s `Applies` column.
fn check_unique_by(
    field: &FieldDescriptor,
    invariant: &FieldInvariant,
    vocabulary: &Vocabulary,
    violations: &mut Vec<Violation>,
) {
    let mut push = |reason| {
        violations.push(Violation {
            subject: field.full_name().to_owned(),
            reason,
        });
    };

    let Kind::Message(element) = field.kind() else {
        // The caller's gate makes this unreachable; refusing quietly here
        // would turn a future gate mistake into keys checked against nothing.
        push(Reason::NotApplicable {
            option: "unique_by",
            applies_to: "repeated message fields",
        });
        return;
    };

    let mut seen = BTreeSet::new();
    for key in &invariant.unique_by {
        if !seen.insert(key.as_str()) {
            push(Reason::DuplicateUniqueKey(key.clone()));
            continue;
        }
        let Some(member) = element.get_field_by_name(key) else {
            push(Reason::UnknownUniqueKey(key.clone()));
            continue;
        };
        if member.is_list() || member.is_map() {
            push(Reason::RepeatedUniqueKey(key.clone()));
            continue;
        }
        // Identity has to be decided on content. An option whose hash stance
        // is "skipped" says the opposite: hashing ignores the field, and
        // canonical sorting reaches it only as a final tiebreaker after every
        // hash-visible one.
        //
        // Keying on such a field defeats the annotation from both ends. Two
        // elements agreeing on everything the hash compares are admitted as
        // distinct — so a hashable collection can hold what a consumer sees as
        // one thing twice, which is what `unique_by` is there to stop — and
        // the sort that is supposed to make duplicates adjacent orders them
        // last by the very field the check treats as primary.
        let member_invariant = vocabulary.field_invariant(&member);
        let excluded_from_hash = member_invariant.as_ref().is_some_and(|member| {
            options::FIELD_OPTIONS.iter().any(|option| {
                option.hash == HashStance::SkippedByHash && (option.render)(member).is_some()
            })
        });
        if excluded_from_hash {
            push(Reason::MetadataUniqueKey(key.clone()));
            continue;
        }
        // A message key is compared for equality, and equal content only
        // compares equal once canonicalization has run. That is the same
        // argument `hashable` makes when it requires `validated`, reached
        // through a different annotation: keying on a type nothing
        // canonicalizes means two elements naming the same thing — differing
        // only in the order of an unordered field inside the key — are
        // admitted as distinct, and the duplicate this exists to reject
        // passes.
        if let Kind::Message(key_type) = member.kind() {
            if !vocabulary
                .message_invariant(&key_type)
                .is_some_and(|invariant| invariant.validated)
            {
                push(Reason::UnvalidatedUniqueKey(key.clone()));
                continue;
            }
        }
        // A key has to be one an element always carries, and which options
        // promise that is a column — `required` promises it through
        // validation, `zero_is_meaningful` through the wire itself. Reading
        // the column rather than naming the options is what once made these
        // two mutually unsatisfiable here: `required` is rejected on a
        // `zero_is_meaningful` field, and a check that tested `required` alone
        // left no way to key on one.
        //
        // A oneof member is excluded whatever it claims. The applicability
        // rules reject `required` there, but this does not rely on that: a key
        // absent whenever a sibling arm holds the value is exactly the case
        // this test is here to catch, and reaching it through a mis-annotated
        // field would be the quietest way to arrive.
        let always_present = !in_declared_oneof(&member)
            && member_invariant.as_ref().is_some_and(|member| {
                options::FIELD_OPTIONS
                    .iter()
                    .any(|option| option.guarantees_presence && (option.render)(member).is_some())
            });
        if !always_present {
            push(Reason::OptionalUniqueKey(key.clone()));
        }
    }
}

/// Whether `field` is a member of a oneof the schema declared.
///
/// proto3 compiles every `optional` field into a synthetic one-field oneof, so
/// the raw question is true of most of the contract and answers nothing. Only a
/// declared oneof means "another arm may hold the value instead".
fn in_declared_oneof(field: &FieldDescriptor) -> bool {
    field
        .containing_oneof()
        .is_some_and(|oneof| !oneof.is_synthetic())
}

fn map_value_kind(field: &FieldDescriptor) -> Option<Kind> {
    let Kind::Message(entry) = field.kind() else {
        return None;
    };
    Some(entry.map_entry_value_field().kind())
}
