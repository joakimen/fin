.DEFAULT_GOAL := build
.PHONY: build check test lint fmt fmt-check install run clean

build: check
	cargo build --locked --release

check: fmt-check lint test

test:
	cargo test --locked

lint:
	cargo clippy --locked --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

install:
	cargo install --locked --path .

run:
	cargo run -- $(ARGS)

clean:
	cargo clean
