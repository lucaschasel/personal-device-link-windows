#!/bin/bash
set -euo pipefail
if [ "$(uname -s)" != Darwin ]; then
  echo 'macOS 应用须在 Mac 或 GitHub macOS runner 上构建。' >&2
  exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=11.0
cd "$(dirname "$0")/../desktop"
# Only a fully built and signed bundle may be reused by the Finder launcher.
rm -f ../.pdl-macos-build-complete
command -v cargo >/dev/null || { echo '请先安装 Rust stable，然后重新打开终端。'; exit 1; }
command -v npm >/dev/null || { echo '请先安装 Node.js LTS（包含 npm），然后重新打开终端。'; exit 1; }
bash ../scripts/build-bluetooth-macos.sh
npm ci --no-audit --no-fund
# Helpers are added below; distribution signing must happen AFTER this script.
# Do not let Tauri notarize a bundle that we are about to modify.
env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE -u APPLE_CERTIFICATE_PASSWORD \
  -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY \
  -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
  npm run desktop:build -- --bundles app --config '{"bundle":{"macOS":{"signingIdentity":"-"}}}' -- --locked
app='src-tauri/target/release/bundle/macos/Personal Device Link.app'
cp ../target/release/bluetooth_bridge "$app/Contents/MacOS/bluetooth_bridge"
cp ../target/release/pdl-rfcomm-mac "$app/Contents/MacOS/pdl-rfcomm-mac"
codesign --force --sign - "$app/Contents/MacOS/bluetooth_bridge"
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
touch ../.pdl-macos-build-complete
echo ''
echo '构建完成。双击以下应用，或执行：'
echo '在源码根目录执行：open "desktop/src-tauri/target/release/bundle/macos/Personal Device Link.app"'
echo '这是本机构建的测试应用，临时签名不等于 Apple 公证。公开分发请使用 package-macos.sh。'
