.PHONY: module FORCE
module: hello_release hello_debug

hello_release: FORCE
	cargo build --release
	cp target/release/alloc_test hello_release

hello_debug: FORCE
	cargo build
	cp target/debug/alloc_test hello_debug

FORCE: