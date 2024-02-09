#!/usr/bin/env bash
cd ../..
gdb build/release/loader.elf -q -ex 'set pagination off' -ex 'connect' -ex 'hb run_main' -ex c -ex 'd 1' -ex 'osv syms -q'