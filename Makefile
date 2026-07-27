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

export RUSTDOCFLAGS := -D warnings

fmt-toolchain := nightly-2026-06-16

define build-and-test
	cargo +$(fmt-toolchain) fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --workspace --all-targets $1 -- -D warnings
	cargo build --workspace
	cargo build --workspace $1
	cargo test --workspace -- --no-capture
	cargo test --workspace $1 -- --no-capture
	cargo doc --workspace --no-deps $1

endef


all:
	$(call build-and-test,--all-features)

ci: rust-install
	$(call build-and-test,--all-features)

fmt:
	cargo +$(fmt-toolchain) fmt --all

rust-install:
	rustup component add clippy rustfmt
	rustup toolchain install $(fmt-toolchain) --profile minimal --component rustfmt

clean:
	rm -rf target
