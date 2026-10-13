#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked --bin bluetooth_bridge
xcrun clang -fobjc-arc -Wall -framework Foundation -framework IOBluetooth \
  -Wl,-sectcreate,__TEXT,__info_plist,scripts/bluetooth-mac.plist \
  scripts/bluetooth-mac.m -o target/release/pdl-rfcomm-mac
codesign --force --sign - --identifier local.personal-device-link.bluetooth-bridge target/release/pdl-rfcomm-mac
echo '构建完成。先在系统蓝牙中配对，再运行：'
echo '等待端：target/release/bluetooth_bridge --listen'
echo '主动端：target/release/bluetooth_bridge --connect 对方蓝牙地址 频道'
