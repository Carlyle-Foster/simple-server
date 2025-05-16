#!/bin/sh
cargo build --example server
rust-gdb -ex run target/debug/examples/server