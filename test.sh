#!/bin/bash
set -e

cargo build --release --package user

# shellcheck disable=SC2046
test_bins=$(find user/testbin/*.rs | sed 's|user/testbin/\(.*\)\.rs|target/riscv64gc-unknown-none-elf/release/\1|')

# init.rs checks for this file to run testrunner instead of sh.
touch /tmp/testmode

# backup original fs.img and create a new one for testing
mv target/fs.img /tmp/fs.img.backup
qemu-img create target/fs.img 256M

# Pass test binaries and the testmode marker as extra files to mkfs.sh.
# shellcheck disable=SC2086
./mkfs.sh $test_bins /tmp/testmode

# start a UDP echo server in the background
python3 -c "
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(('', 26100))
while True:
    data, addr = s.recvfrom(2048)
    s.sendto(data, addr)
" &
ECHO_PID=$!


if ! cargo run --release; then
  echo "test failed"
  # restore original fs.img
  mv /tmp/fs.img.backup target/fs.img
  kill $ECHO_PID 2>/dev/null || true
  exit 1
fi

kill $ECHO_PID 2>/dev/null || true

echo "test passed"
# restore original fs.img
mv /tmp/fs.img.backup target/fs.img
