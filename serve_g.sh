#!/bin/sh
cargo build --example server_g
rust-gdb -ex run target/debug/examples/server_g