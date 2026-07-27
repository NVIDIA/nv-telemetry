#
# Build and test everything.
#
# Lint levels live in the workspace manifest rather than in the flags below, so
# an editor reports exactly what this pipeline will. The only thing added here
# is promoting warnings to errors, which is wanted in CI but not while editing.
#

export RUSTDOCFLAGS := -D warnings

define build-and-test
	cargo fmt --all -- --check
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

rust-install:
	rustup component add clippy rustfmt

clean:
	rm -rf target
