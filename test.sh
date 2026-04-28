#!/bin/bash
set -e

cargo build --release --package user

# shellcheck disable=SC2046
test_bins=$(find user/testbin/*.rs | sed 's|user/testbin/\(.*\)\.rs|target/riscv64gc-unknown-none-elf/release/_\1|')

# init.rs checks for this file to run testrunner instead of sh.
touch /tmp/testmode

# backup original fs.img and create a new one for testing
mv target/fs.img /tmp/fs.img.backup
qemu-img create target/fs.img 256M

# Pass test binaries and the testmode marker as extra files to mkfs.sh.
# shellcheck disable=SC2086
./mkfs.sh $test_bins /tmp/testmode

if ! cargo run --release; then
  echo "test failed"
  # restore original fs.img
  mv /tmp/fs.img.backup target/fs.img
  exit 1
fi

echo "test passed"
# restore original fs.img
mv /tmp/fs.img.backup target/fs.img
