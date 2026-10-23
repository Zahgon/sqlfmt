.PHONY: check
check: lint unit

.PHONY: unit
unit:
	cargo test

.PHONY: lint
lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

.PHONY: release
release:
	cargo build --release
