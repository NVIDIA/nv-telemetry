#
# Build and test everything.
#
# Lint levels live in the workspace manifest rather than in the flags below, so
# an editor reports exactly what this pipeline will. The only thing added here
# is promoting warnings to errors, which is wanted in CI but not while editing.
#
# rustfmt.toml uses a nightly-only option, and stable rustfmt would ignore it
# with a warning rather than an error. Every format step therefore names the
# toolchain, so a plain `cargo fmt` cannot quietly disagree with the pipeline.
#
# The schema is checked by buf, configured in buf.yaml. buf is a checker only:
# the descriptor set is built by protox from schema/build.rs, so `cargo build`
# needs no external binary and a consumer of the data plane pulls only prost.
#

export RUSTDOCFLAGS := -D warnings

fmt-toolchain := nightly-2026-06-16

buf ?= buf

# Pinned so that `make fmt` locally and the format check in CI agree.
# Set empty to disable the check.
buf-version ?= 1.72.0

# Compatibility is judged against this ref. Overridable so a release branch can
# be compared against its own line rather than against main.
proto-baseline ?= origin/main

# Several targets share a name with a directory in the workspace, `codegen`
# being the one that matters. Without this, make sees the directory, decides
# the target is up to date, and silently does nothing.
.PHONY: all ci fmt bench codegen check-codegen proto-lint proto-breaking \
	check-proto-format require-buf rust-install clean

# `--locked` throughout: generated output is byte-compared, and it is produced
# by prost-build and prettyplease. Without the lockfile held authoritative, a
# patch bump to either rewrites the generated tree and surfaces as "generated
# files disagree with the schema" on a pull request that touched neither.
define build-and-test
	cargo +$(fmt-toolchain) fmt --all -- --check
	+$(MAKE) check-proto-format
	+$(MAKE) proto-lint
	+$(MAKE) proto-breaking
	+$(MAKE) check-codegen
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo clippy --locked --workspace --all-targets $1 -- -D warnings
	cargo build --locked --workspace
	cargo build --locked --workspace $1
	cargo test --locked --workspace -- --no-capture
	cargo test --locked --workspace $1 -- --no-capture
	cargo doc --locked --workspace --no-deps $1

endef


all:
	$(call build-and-test,--all-features)

ci: rust-install
	$(call build-and-test,--all-features)

fmt: require-buf
	cargo +$(fmt-toolchain) fmt --all
	$(buf) format -w

# Style and consistency rules over the schema. These cover what our own
# compiler deliberately does not: naming, package/directory correspondence,
# and enum value prefixes, which matter because proto enum values are scoped to
# the enclosing package rather than to their enum.
proto-lint: require-buf
	$(buf) lint

check-proto-format: require-buf
	$(buf) format --diff --exit-code

# Additive-only evolution. buf owns this rather than the contract lock, because
# it already covers reserved ranges, enum changes, JSON names, and oneof moves,
# and because FIELD_SAME_CARDINALITY catches a field quietly losing explicit
# presence, which is how fabricated zeros would come back.
#
# Skipped, loudly, until the schema exists at the baseline. That is a real gap
# while it lasts: the first push of the schema is compared against nothing.
#
# Deciding when to skip is the whole difficulty, and it lives in the script
# rather than here. Two earlier attempts got it wrong in the same direction:
# one tested for a hardcoded schema directory and stopped finding it the day
# the schema moved, the other matched buf's output for "no .proto files" and
# could not tell an empty baseline from a deleted contract. Both reported
# success while checking nothing.
proto-breaking: require-buf
	BUF=$(buf) bash tools/proto-breaking.sh $(proto-baseline)

require-buf:
	@BUF=$(buf) bash tools/require-buf.sh $(buf-version)

# Generated output is checked in, so a schema change and its generated output
# land in the same reviewable diff and no downstream build needs a protobuf
# compiler. `check-codegen` runs inside the pipeline above, ahead of the build
# steps, so a schema edit without regeneration fails immediately rather than
# surfacing later as a confusing compile error.
codegen:
	cargo run --locked -p nv-telemetry-codegen -- generate

check-codegen:
	cargo run --locked -p nv-telemetry-codegen -- --check

# Instruction counts under Valgrind, so results do not depend on machine
# load. Needs valgrind and a gungraun-runner matching the gungraun version
# the lockfile resolved. CI compares these against the merge-base; locally
# they are absolute numbers unless a baseline was saved.
bench:
	cargo bench --workspace --all-features

rust-install:
	rustup component add clippy rustfmt
	rustup toolchain install $(fmt-toolchain) --profile minimal --component rustfmt

clean:
	rm -rf target
