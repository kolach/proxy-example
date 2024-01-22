.PHONY: test build

test:
	cargo test

build: test
	cargo build
