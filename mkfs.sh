#!/bin/bash

cargo build --release --package user

pushd mkfs
cargo run --release --target "$(rustc -vV | grep host | cut -d' ' -f2)" -- ../target/fs.img $(ls ../user/bin/*.rs | sed 's|../user/bin/\(.*\)\.rs|../target/riscv64gc-unknown-none-elf/release/\1|') ../LICENSE
popd
