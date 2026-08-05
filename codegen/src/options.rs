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
//!
//! Each option is defined exactly once, as a row of `FIELD_OPTIONS`,
//! `MESSAGE_OPTIONS`, or `ONEOF_OPTIONS`: its declared shape, the fields
//! it may be written on, what it asserts about the zero value and about hash
//! visibility, how the contract lock renders it, and what the canary must
//! read back. The reader, the lint, the lock, and the canary all consume the
//! rows, so the decisions an option demands are columns the compiler will not
//! build without — not steps of a checklist a review has to catch. The
//! vocabulary previously kept those decisions in parallel lists, and every
//! way two of them could disagree was found to have happened at least once.

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

/// The package the annotation vocabulary lives in.
const VOCABULARY_PACKAGE: &str = "nv.telemetry.options.v1";

/// Whether `package` is part of the annotation vocabulary.
///
/// Matched as a prefix rather than for equality, for the reason
/// [`crate::is_contract_package`] gives: an option declared in a future
/// `nv.telemetry.options.v1.experimental` would otherwise be silently
/// unchecked, which is the hole the caller exists to close.
fn is_vocabulary_package(package: &str) -> bool {
    crate::package_within(package, VOCABULARY_PACKAGE)
}

/// Every option this compiler reads. An extension declared in the vocabulary
/// package and absent here is one an author can write and nothing will act on.
const READ: &[&str] = &[FIELD_INVARIANT, MESSAGE_INVARIANT, ONEOF_INVARIANT];

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
    /// The vocabulary declares an option this compiler never reads, so writing
    /// it would do nothing at all.
    NotRead,
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
            VocabularyErrorKind::NotRead => write!(
                f,
                "the vocabulary declares `{}`, which this compiler does not \
                 read; a schema could carry it on every declaration and no \
                 generated code would act on it, while a review would take it \
                 for an enforced constraint. Read it, or remove it",
                self.name
            ),
        }
    }
}

impl std::error::Error for VocabularyError {}

/// Shapes the compiler reads out of an annotation.
///
/// Cardinality is part of the shape, not just the scalar type. A bound widened
/// from `optional uint32` to `repeated uint32` keeps its `Kind`, so a check on
/// type alone would pass while the reader — which asks for a single value —
/// returned `None` and switched the bound off. Presence is checked for the same
/// reason in the other direction: dropping `optional` from a bound collapses
/// `max_items: 0` back into "no bound", which is a bug this vocabulary has
/// already had once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Bool,
    OptionalUint32,
    RepeatedString,
}

impl Shape {
    fn accepts(self, field: &FieldDescriptor) -> bool {
        match self {
            Self::Bool => matches!(field.kind(), Kind::Bool) && !field.is_list(),
            Self::OptionalUint32 => {
                matches!(field.kind(), Kind::Uint32)
                    && !field.is_list()
                    && field.supports_presence()
            }
            Self::RepeatedString => matches!(field.kind(), Kind::String) && field.is_list(),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::OptionalUint32 => "optional uint32",
            Self::RepeatedString => "repeated string",
        }
    }
}

/// The fields an option can be written on.
///
/// One column of [`FieldOption`]. The lint rejects an option whose carrier the
/// variant does not permit, wording the rejection with
/// [`description`](Self::description) — so a new option states where it
/// applies by picking a variant, not by contributing a hand-written rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Applies {
    /// Any field. Options constrained by context rather than by shape.
    Anything,
    /// `double` and `float` fields; per element when repeated.
    Floats,
    /// `string` and `bytes` fields; per element when repeated.
    StringsAndBytes,
    /// Enum fields; per element when repeated.
    Enums,
    /// Repeated fields — and not maps, which have no wire order, so there is
    /// nothing for canonicalization to sort.
    Lists,
    /// Repeated and map fields.
    ListsAndMaps,
    /// Scalars with proto3 implicit presence: the only fields where "the zero
    /// value is data" adds information, because everywhere else absence is
    /// already expressible.
    ImplicitScalars,
    /// Fields that can be absent. A repeated field is empty rather than
    /// absent, and an implicit scalar is zero rather than absent, so a demand
    /// about absence means nothing on either.
    FieldsWithPresence,
    /// Repeated message fields.
    MessageLists,
}

impl Applies {
    pub(crate) fn permits(self, field: &FieldDescriptor) -> bool {
        match self {
            Self::Anything => true,
            Self::Floats => matches!(field.kind(), Kind::Double | Kind::Float),
            Self::StringsAndBytes => matches!(field.kind(), Kind::String | Kind::Bytes),
            Self::Enums => matches!(field.kind(), Kind::Enum(_)),
            Self::Lists => field.is_list(),
            Self::ListsAndMaps => field.is_list() || field.is_map(),
            Self::ImplicitScalars => {
                !field.supports_presence() && !field.is_list() && !field.is_map()
            }
            Self::FieldsWithPresence => field.supports_presence(),
            Self::MessageLists => field.is_list() && matches!(field.kind(), Kind::Message(_)),
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Anything => "any field",
            Self::Floats => "double and float fields",
            Self::StringsAndBytes => "string and bytes fields",
            Self::Enums => "enum fields",
            Self::Lists => "repeated fields",
            Self::ListsAndMaps => "repeated and map fields",
            Self::ImplicitScalars => "scalars that have no presence of their own",
            Self::FieldsWithPresence => {
                "fields that can be absent; a repeated field is empty rather \
                 than absent, and an implicit scalar is zero rather than absent"
            }
            Self::MessageLists => "repeated message fields",
        }
    }
}

/// What an option asserts about the field's zero value.
///
/// Two options taking opposite stances on one field contradict each other, and
/// the lint derives that from this column rather than knowing the pairs: when
/// the vocabulary gains an option, its row states a stance and every conflict
/// with existing options falls out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZeroStance {
    Indifferent,
    /// The zero value is a real observation.
    ZeroIsData,
    /// The zero value is invalid — an empty string, an UNSPECIFIED enum.
    ZeroIsInvalid,
}

/// What an option asserts about the field's visibility to content hashing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HashStance {
    Indifferent,
    /// Hashed unconditionally: the field has no absent state, so there is no
    /// "present" to test.
    AlwaysHashed,
    /// Skipped by hashing: the field records how a fact was collected rather
    /// than what was observed.
    SkippedByHash,
}

/// What the canary must read back for one option.
struct Probe {
    /// The canary field (or, for message options, the canary itself) whose
    /// annotation carries the option.
    field: &'static str,
    /// Whether the annotation read back as the canary declares it.
    holds: fn(&FieldInvariant) -> bool,
    /// The failure, phrased for the build error.
    or: &'static str,
}

/// One option of the field vocabulary — the single definition everything else
/// derives from.
///
/// The reader checks `shape`, the lint checks `applies` and derives
/// contradictions from `zero` and `hash`, the vacuousness rule under
/// `max_items: 0` reads `per_element`, the `unique_by` key rule reads
/// `guarantees_presence` and `hash`, the contract lock renders through
/// `render`, and the canary self-test runs `canary`. A row missing any column
/// does not compile, which is the point: the alternative was parallel lists,
/// and every way two of them could disagree eventually did.
pub(crate) struct FieldOption {
    pub(crate) name: &'static str,
    shape: Shape,
    pub(crate) applies: Applies,
    pub(crate) zero: ZeroStance,
    pub(crate) hash: HashStance,
    /// Describes elements of a collection (their values or their arrangement),
    /// so under `max_items: 0` it describes nothing.
    pub(crate) per_element: bool,
    /// A field carrying this option always holds a value once validated, so it
    /// can serve as a `unique_by` key.
    pub(crate) guarantees_presence: bool,
    /// The option's contract-lock rendering; `None` when not set. Doubles as
    /// "is this option written on the field", which keeps set-ness and
    /// rendering incapable of disagreeing.
    pub(crate) render: fn(&FieldInvariant) -> Option<String>,
    canary: Probe,
}

/// The field vocabulary. Row order is declaration order in
/// `annotations.proto`, and diagnostics preserve it.
pub(crate) const FIELD_OPTIONS: &[FieldOption] = &[
    FieldOption {
        name: "zero_is_meaningful",
        shape: Shape::Bool,
        applies: Applies::ImplicitScalars,
        zero: ZeroStance::ZeroIsData,
        hash: HashStance::AlwaysHashed,
        per_element: false,
        guarantees_presence: true,
        render: |invariant| {
            invariant
                .zero_is_meaningful
                .then(|| "zero_is_meaningful".to_owned())
        },
        canary: Probe {
            field: "revision",
            holds: |invariant| invariant.zero_is_meaningful,
            or: "the canary's `revision` did not read `zero_is_meaningful` back as declared",
        },
    },
    FieldOption {
        name: "finite",
        shape: Shape::Bool,
        applies: Applies::Floats,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        render: |invariant| invariant.finite.then(|| "finite".to_owned()),
        canary: Probe {
            field: "reading",
            holds: |invariant| invariant.finite,
            or: "the canary's `reading` did not read `finite` back as declared",
        },
    },
    FieldOption {
        name: "required",
        shape: Shape::Bool,
        // This is what makes `required` on a repeated field an error: absence
        // is what validators reject, and a repeated field is never absent, so
        // the annotation there would be a minimum item count wearing the wrong
        // name — a rule this vocabulary deliberately does not have.
        applies: Applies::FieldsWithPresence,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: false,
        guarantees_presence: true,
        render: |invariant| invariant.required.then(|| "required".to_owned()),
        canary: Probe {
            field: "reading",
            holds: |invariant| invariant.required,
            or: "the canary's `reading` did not read `required` back as declared",
        },
    },
    FieldOption {
        name: "unordered",
        shape: Shape::Bool,
        applies: Applies::Lists,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        render: |invariant| invariant.unordered.then(|| "unordered".to_owned()),
        canary: Probe {
            field: "labels",
            holds: |invariant| invariant.unordered,
            or: "the canary's `labels` did not read `unordered` back as declared",
        },
    },
    FieldOption {
        name: "max_items",
        shape: Shape::OptionalUint32,
        applies: Applies::ListsAndMaps,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: false,
        guarantees_presence: false,
        render: |invariant| {
            invariant
                .max_items
                .map(|bound| format!("max_items={bound}"))
        },
        canary: Probe {
            field: "labels",
            holds: |invariant| invariant.max_items == Some(4),
            or: "the canary's `labels` did not read `max_items: 4` back as declared",
        },
    },
    FieldOption {
        name: "max_len",
        shape: Shape::OptionalUint32,
        applies: Applies::StringsAndBytes,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        render: |invariant| invariant.max_len.map(|bound| format!("max_len={bound}")),
        canary: Probe {
            field: "labels",
            holds: |invariant| invariant.max_len == Some(32),
            or: "the canary's `labels` did not read `max_len: 32` back as declared",
        },
    },
    FieldOption {
        name: "collection_metadata",
        shape: Shape::Bool,
        // No shape constraint: what it excludes from hashing is decided by
        // reachability from a hashable message, which is context the lint
        // checks, not shape.
        applies: Applies::Anything,
        zero: ZeroStance::Indifferent,
        hash: HashStance::SkippedByHash,
        per_element: false,
        guarantees_presence: false,
        render: |invariant| {
            invariant
                .collection_metadata
                .then(|| "collection_metadata".to_owned())
        },
        canary: Probe {
            field: "trace",
            holds: |invariant| invariant.collection_metadata,
            or: "the canary's `trace` did not read `collection_metadata` back as declared",
        },
    },
    FieldOption {
        name: "non_empty",
        shape: Shape::Bool,
        // Per element on repeated strings and bytes, exactly as `max_len` is —
        // `Subject.scope` relies on that, its list being legitimately empty
        // while an element never is. "The list is not empty" is a minimum item
        // count, a different rule this vocabulary does not have.
        applies: Applies::StringsAndBytes,
        zero: ZeroStance::ZeroIsInvalid,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        render: |invariant| invariant.non_empty.then(|| "non_empty".to_owned()),
        canary: Probe {
            field: "name",
            holds: |invariant| invariant.non_empty,
            or: "the canary's `name` did not read `non_empty` back as declared",
        },
    },
    FieldOption {
        name: "reject_unspecified",
        shape: Shape::Bool,
        applies: Applies::Enums,
        zero: ZeroStance::ZeroIsInvalid,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        render: |invariant| {
            invariant
                .reject_unspecified
                .then(|| "reject_unspecified".to_owned())
        },
        canary: Probe {
            field: "level",
            holds: |invariant| invariant.reject_unspecified,
            or: "the canary's `level` did not read `reject_unspecified` back as declared",
        },
    },
    FieldOption {
        name: "unique_by",
        shape: Shape::RepeatedString,
        applies: Applies::MessageLists,
        zero: ZeroStance::Indifferent,
        hash: HashStance::Indifferent,
        per_element: true,
        guarantees_presence: false,
        // Keys render in declaration order, not sorted: they are a tuple, and
        // reordering one is a change worth seeing even though it does not
        // change which elements collide.
        render: |invariant| {
            (!invariant.unique_by.is_empty())
                .then(|| format!("unique_by=[{}]", invariant.unique_by.join(",")))
        },
        // A repeated string is a third reflection shape, and the way it fails
        // is the quietest of the three: an unread list comes back empty, which
        // is exactly how "no uniqueness constraint" is spelled.
        canary: Probe {
            field: "elements",
            holds: |invariant| invariant.unique_by == ["id"],
            or: "the canary's `elements` did not read `unique_by: [\"id\"]` back as declared",
        },
    },
];

/// One option of the message vocabulary.
///
/// `holds` and `render` take the invariant by value because it is small and
/// `Copy`; the field vocabulary passes by reference only because `unique_by`
/// makes [`FieldInvariant`] own a list.
pub(crate) struct MessageOption {
    pub(crate) name: &'static str,
    shape: Shape,
    pub(crate) render: fn(MessageInvariant) -> Option<String>,
    holds: fn(MessageInvariant) -> bool,
    or: &'static str,
}

/// The message vocabulary. The canary probes run against the canary message's
/// own annotation.
pub(crate) const MESSAGE_OPTIONS: &[MessageOption] = &[
    MessageOption {
        name: "validated",
        shape: Shape::Bool,
        render: |invariant| invariant.validated.then(|| "validated".to_owned()),
        holds: |invariant| invariant.validated,
        or: "the canary did not read `validated` back as declared",
    },
    MessageOption {
        name: "hashable",
        shape: Shape::Bool,
        render: |invariant| invariant.hashable.then(|| "hashable".to_owned()),
        holds: |invariant| invariant.hashable,
        or: "the canary did not read `hashable` back as declared",
    },
    MessageOption {
        name: "max_depth",
        shape: Shape::OptionalUint32,
        render: |invariant| {
            invariant
                .max_depth
                .map(|bound| format!("max_depth={bound}"))
        },
        holds: |invariant| invariant.max_depth == Some(8),
        or: "the canary did not read `max_depth: 8` back as declared",
    },
];

/// One option of the oneof vocabulary.
pub(crate) struct OneofOption {
    pub(crate) name: &'static str,
    shape: Shape,
    holds: fn(OneofInvariant) -> bool,
    or: &'static str,
}

/// The oneof vocabulary. The probes run against the canary's `probe` oneof.
pub(crate) const ONEOF_OPTIONS: &[OneofOption] = &[OneofOption {
    name: "required",
    shape: Shape::Bool,
    holds: |invariant| invariant.required,
    or: "the canary's oneof `required` read back false",
}];

/// Constraints declared on a single field.
///
/// Bounds are optional rather than zero-defaulted, because `max_items: 0` is a
/// legitimate bound and collapsing it into "unset" would reproduce, inside the
/// compiler, the presence bug the compiler exists to prevent.
// The fields mirror the annotation message one for one. Collapsing them into
// flags would put the compiler's model out of step with the schema it reads,
// which is the drift every other rule here exists to prevent.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// The field is collection metadata, so content hashing skips it.
    pub collection_metadata: bool,
    /// A string or bytes value must not be empty; per element when repeated.
    pub non_empty: bool,
    /// An enum field must not carry the zero value.
    pub reject_unspecified: bool,
    /// Fields of the element type that together identify an element of this
    /// repeated field. Empty means no uniqueness constraint.
    pub unique_by: Vec<String>,
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
    /// A vocabulary field that is used in-tree fails earlier and louder — the
    /// schema build cannot compile the use sites against a renamed
    /// declaration — so these checks guard the field with no uses yet, which
    /// is exactly the one whose silent default would otherwise go unnoticed.
    ///
    /// # Errors
    ///
    /// Returns [`VocabularyError`] describing the first annotation that is
    /// absent, incomplete, unreadable as declared, or whose values were lost.
    pub fn resolve(pool: &DescriptorPool) -> Result<Self, VocabularyError> {
        let field_shapes: Vec<(&'static str, Shape)> = FIELD_OPTIONS
            .iter()
            .map(|option| (option.name, option.shape))
            .collect();
        let message_shapes: Vec<(&'static str, Shape)> = MESSAGE_OPTIONS
            .iter()
            .map(|option| (option.name, option.shape))
            .collect();
        let oneof_shapes: Vec<(&'static str, Shape)> = ONEOF_OPTIONS
            .iter()
            .map(|option| (option.name, option.shape))
            .collect();

        let vocabulary = Self {
            field: extension(pool, FIELD_INVARIANT, &field_shapes)?,
            message: extension(pool, MESSAGE_INVARIANT, &message_shapes)?,
            oneof: extension(pool, ONEOF_INVARIANT, &oneof_shapes)?,
        };
        check_all_options_are_read(pool)?;
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
            collection_metadata: boolean(&annotation, "collection_metadata"),
            non_empty: boolean(&annotation, "non_empty"),
            reject_unspecified: boolean(&annotation, "reject_unspecified"),
            unique_by: strings(&annotation, "unique_by"),
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

    /// Names of the field options this compiler reads, in declaration order.
    ///
    /// Exposed so a test can check the canary exercises every one of them
    /// without keeping a second list that drifts from this one.
    #[must_use]
    pub fn field_option_names() -> Vec<&'static str> {
        FIELD_OPTIONS.iter().map(|option| option.name).collect()
    }

    /// Names of the message options this compiler reads.
    #[must_use]
    pub fn message_option_names() -> Vec<&'static str> {
        MESSAGE_OPTIONS.iter().map(|option| option.name).collect()
    }

    /// Names of the oneof options this compiler reads.
    #[must_use]
    pub fn oneof_option_names() -> Vec<&'static str> {
        ONEOF_OPTIONS.iter().map(|option| option.name).collect()
    }

    /// Whether `field`'s annotation sets `option` to something other than its
    /// default.
    ///
    /// Used by the canary coverage test rather than by generation: the typed
    /// readers above answer what an option *says*, and this answers whether it
    /// was written at all.
    #[must_use]
    pub fn field_option_is_set(&self, field: &FieldDescriptor, option: &str) -> bool {
        read(&self.field, &field.options()).is_some_and(|annotation| is_set(&annotation, option))
    }

    /// Whether `message`'s annotation sets `option`.
    #[must_use]
    pub fn message_option_is_set(&self, message: &MessageDescriptor, option: &str) -> bool {
        read(&self.message, &message.options())
            .is_some_and(|annotation| is_set(&annotation, option))
    }

    /// Runs every canary probe the option tables declare.
    ///
    /// The probes live on the rows, so an option cannot be added without
    /// stating what the canary declares for it, and the assertion cannot be
    /// deleted while the field stays — the failure mode the hand-written
    /// predecessor of this function had twice.
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
        for option in MESSAGE_OPTIONS {
            if !(option.holds)(message) {
                return Err(fault(option.or));
            }
        }

        for option in FIELD_OPTIONS {
            let invariant = canary
                .get_field_by_name(option.canary.field)
                .and_then(|probed| self.field_invariant(&probed))
                .ok_or_else(|| fault(option.canary.or))?;
            if !(option.canary.holds)(&invariant) {
                return Err(fault(option.canary.or));
            }
        }

        // OneofOptions ride a different options message than FieldOptions and
        // MessageOptions, so the round trip has to cover that path separately.
        let probe = canary
            .oneofs()
            .find(|oneof| oneof.name() == "probe")
            .ok_or_else(|| fault("the canary has no `probe` oneof"))?;
        let probe = self
            .oneof_invariant(&probe)
            .ok_or_else(|| fault("the canary's `probe` carries no annotation"))?;
        for option in ONEOF_OPTIONS {
            if !(option.holds)(probe) {
                return Err(fault(option.or));
            }
        }

        Ok(())
    }
}

/// Rejects an option the vocabulary declares and this compiler never reads.
///
/// [`Vocabulary::resolve`] checks the other direction — that every option the
/// compiler reads is declared, with a shape it can read. This is the mirror,
/// and it fails more quietly than its twin: a renamed option is at least
/// reported by the check above, while an option nobody reads produces no
/// error anywhere. It can be written on every declaration in the contract, the
/// lock will not record it because the lock reads through this type, and the
/// only trace of it will be schema text that a reviewer takes for an enforced
/// rule.
///
/// This is how a withdrawn option comes back. `docs/EXTENSIONS.md` records
/// 52004 as withdrawn precisely because it was declared before anything read
/// it.
fn check_all_options_are_read(pool: &DescriptorPool) -> Result<(), VocabularyError> {
    let mut unread: Vec<String> = pool
        .all_extensions()
        .filter(|extension| is_vocabulary_package(extension.parent_file().package_name()))
        .map(|extension| extension.full_name().to_owned())
        .filter(|name| !READ.contains(&name.as_str()))
        .collect();

    // Sorted so the first one reported does not depend on pool iteration
    // order, which would make the error message vary between runs.
    unread.sort();
    match unread.first() {
        Some(name) => Err(VocabularyError {
            name: name.clone(),
            kind: VocabularyErrorKind::NotRead,
        }),
        None => Ok(()),
    }
}

fn extension(
    pool: &DescriptorPool,
    name: &str,
    shape: &[(&'static str, Shape)],
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

    for &(field, shape) in shape {
        let declared = annotation
            .get_field_by_name(field)
            .ok_or_else(|| fault(VocabularyErrorKind::MissingField { field }))?;

        if !shape.accepts(&declared) {
            return Err(fault(VocabularyErrorKind::WrongType {
                field,
                expected: shape.name(),
            }));
        }
    }

    // And the reverse: every field the annotation declares must be one the
    // table names. `check_all_options_are_read` makes this check for whole
    // extensions; this is the same hole one level down, and the likelier of
    // the two, because adding a field to `FieldInvariant` is a far more
    // ordinary edit than adding an `extend` block.
    //
    // Without it, a field added here and nowhere else can be written on every
    // declaration in the contract while nothing reads it, nothing generates
    // from it, and the lock does not record it — leaving schema text that a
    // reviewer takes for an enforced rule.
    for declared in annotation.fields() {
        if !shape.iter().any(|&(field, _)| field == declared.name()) {
            return Err(VocabularyError {
                name: declared.full_name().to_owned(),
                kind: VocabularyErrorKind::NotRead,
            });
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

/// Whether `field` carries a value in `annotation` at all.
///
/// For an implicit-presence field this means "not the default", which is what
/// the coverage check wants: a canary declaring `non_empty: false` exercises
/// nothing.
fn is_set(annotation: &DynamicMessage, field: &str) -> bool {
    annotation
        .descriptor()
        .get_field_by_name(field)
        .is_some_and(|descriptor| annotation.has_field(&descriptor))
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

fn strings(annotation: &DynamicMessage, field: &str) -> Vec<String> {
    annotation
        .get_field_by_name(field)
        .and_then(|value| {
            value.as_list().map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(ToOwned::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}
