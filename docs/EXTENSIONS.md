# Protobuf extension numbers

`nv-telemetry` annotates its schema with custom options, which are extensions
of `google.protobuf.*Options`. This file is the registry for the numbers it
uses.

## Allocation

| Number | Extends          | Option                                     |
| ------ | ---------------- | ------------------------------------------ |
| 52001  | `FieldOptions`   | `nv.telemetry.options.v1.field_invariant`   |
| 52002  | `MessageOptions` | `nv.telemetry.options.v1.message_invariant` |
| 52003  | `OneofOptions`   | `nv.telemetry.options.v1.oneof_invariant`   |
| 52004  | `EnumOptions`    | `nv.telemetry.options.v1.enum_invariant`    |

`nv-telemetry` claims 52000–52099 for every extendee.

## Why these numbers are not, and cannot be, globally unique

Protobuf reserves 50000–99999 for use inside a single organization and
explicitly does not treat those numbers as globally unique; the public
extension registry allocates numbers outside that range. A number in the 52000
block therefore cannot be publicly registered, and no amount of documentation
makes it collision-proof outside NVIDIA.

That is an accepted trade, not an oversight. Registering globally would mean
renumbering, and the number is baked into every serialized `FileDescriptorSet`
this project ships — including stored history and anything a consumer has
cached — so the cost rises with every release. If the schema is ever published
for use outside NVIDIA, that is the moment to take a registered number, and the
cost of doing it then should be weighed against doing it now.

## Coordination

Extension numbers are namespaced **per extendee**, not globally. `FieldOptions`
52001 and `MessageOptions` 52001 are different slots, so a claim that does not
name the extendee cannot prevent the collision it is written to prevent. The
table above names both.

Numbers must be coordinated with every other NVIDIA schema that could share a
descriptor pool with this one. `infra-controller` already uses `MethodOptions`
51000 and `FieldOptions` 51001 for its admin-CLI annotations, which is why this
project starts at 52000 rather than at the bottom of the range.

A collision does not fail where it is made. Options are identified on the wire
by number alone, so two schemas claiming one number produce either a
pool-construction failure or a silently misdecoded annotation in whichever tool
first loads both — typically a reflection client, a language binding, or a
service owned by neither team.

## Adding one

1. Add a row above, naming the extendee.
2. Declare the `extend` block in
   `schema/proto/nv/telemetry/options/v1/annotations.proto`.
3. If the compiler reads it, add its shape to the tables in
   `codegen/src/options.rs`, so a rename or a widened integer is a build error
   rather than a silently defaulted value.
4. If it carries an invariant, extend the canary in `annotations.proto` and the
   assertions in `Vocabulary::check_canary`, so the annotation self-test covers
   it. A canary field nothing asserts about proves nothing.
5. Give numeric options explicit presence (`optional uint32`). A bare proto3
   scalar cannot distinguish a bound of zero from no bound, which is the same
   collapse the contract rules exist to prevent.

## Removing one

Reserve both halves: `reserved 6;` **and** `reserved "max_len";`. A number
reservation alone leaves the name free, so a later field can take the old name
at a new number and collide with stored JSON and text-format data, which is
exactly where old and new tooling meet.

This applies to numbers that have been released. A number that only ever
existed in an unmerged branch has no encoder to protect and no decoder to
confuse, so removing it is a plain edit: renumber densely and reserve nothing.
Reserving there records a history no consumer can observe, and burns numbers
in a vocabulary still being designed.
