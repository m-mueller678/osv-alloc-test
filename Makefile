.PHONY: module FORCE
module: hello_release hello_debug virtual_alloc_c

hello_release: FORCE
	cargo build --bin virtual_alloc --release --features tikv-jemallocator
	cp target/release/virtual_alloc hello_release

hello_debug: FORCE
	cargo build --bin virtual_alloc --features tikv-jemallocator
	cp target/debug/virtual_alloc hello_debug

check_fmt:
	cargo fmt
	cargo check --lib
	cargo check --bin virtual_alloc --lib --all-features
	cargo check --bin virtual_alloc --features tikv-jemallocator

libvirtual_alloc_debug.a: FORCE
	cargo build --lib --features hash_map_debug
	cp target/debug/libvirtual_alloc.a $@

libvirtual_alloc_release.a: FORCE
	cargo build --lib --release
	cp target/release/libvirtual_alloc.a $@

virtual_alloc_c: libvirtual_alloc_debug.a main.c
	gcc main.c libvirtual_alloc_debug.a -g -o $@

FORCE: