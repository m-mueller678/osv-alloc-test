.PHONY: module FORCE
module: hello_release hello_debug

hello_release: FORCE
	cargo build --release
	cp target/release/alloc_test hello_release

hello_debug: FORCE
	cargo build
	cp target/debug/alloc_test hello_debug

check_fmt:
	cargo fmt
	cargo check --lib
	cargo check --bin virtual_alloc --lib --all-features
	cargo check --bin virtual_alloc --features tikv-jemallocator

FORCE: