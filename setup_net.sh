#!/bin/bash
set -e

TAP_DEV="octopos0"
TAP_IP="192.168.10.1/24"
USER_NAME=$(whoami)

echo "Setting up $TAP_DEV for $USER_NAME..."

sudo ip tuntap add dev $TAP_DEV mode tap user "$USER_NAME"
sudo ip addr add $TAP_IP dev $TAP_DEV
sudo ip link set $TAP_DEV up

echo "done"
