#!/bin/bash
# Run on a native macOS runner. No signing credentials are needed for test DMGs.
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:-test}"
case "$mode" in test|signed) ;; *) echo 'Usage: bash scripts/package-macos.sh [test|signed]' >&2; exit 2 ;; esac
if [ "$(uname -s)" != Darwin ]; then echo 'This package must be built on macOS.' >&2; exit 1; fi
if [ "$mode" = signed ]; then
  # Validate before the expensive build. Never silently fall back to a test package.
  for key in PDL_CERTIFICATE_BASE64 PDL_CERTIFICATE_PASSWORD PDL_SIGNING_IDENTITY PDL_APPLE_ID PDL_APPLE_PASSWORD PDL_APPLE_TEAM_ID; do
    if [ -z "${!key:-}" ]; then echo "Missing signing setting: $key" >&2; exit 1; fi
  done
  case "$PDL_SIGNING_IDENTITY" in 'Developer ID Application: '*) ;; *) echo 'A Developer ID Application identity is required.' >&2; exit 1 ;; esac
fi
bash scripts/build-macos-gui.sh
app="$PWD/desktop/src-tauri/target/release/bundle/macos/Personal Device Link.app"
version="$(node -p "require('./desktop/package.json').version")"
arch="$(uname -m)"
case "$arch" in arm64|x86_64) ;; *) echo "Unsupported architecture: $arch" >&2; exit 1 ;; esac
mkdir -p dist
output="$PWD/dist/personal-device-link-${version}-macos-${arch}-${mode}.dmg"
# mktemp owns this directory; cleanup never touches the checkout or user keychains.
work="$(mktemp -d "${TMPDIR:-/tmp}/pdl-package.XXXXXX")"
keychain="$work/signing.keychain-db"
cleanup() {
  if [ -f "$keychain" ]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
if [ "$mode" = signed ]; then
  # Keep credentials in a temporary keychain; do not enable shell tracing.
  printf '%s' "$PDL_CERTIFICATE_BASE64" | base64 --decode > "$work/certificate.p12"
  keychain_password="$(openssl rand -hex 24)"
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 21600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  security import "$work/certificate.p12" -k "$keychain" -P "$PDL_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
  rm -f "$work/certificate.p12"
  xcrun notarytool store-credentials pdl-release --keychain "$keychain" \
    --apple-id "$PDL_APPLE_ID" --team-id "$PDL_APPLE_TEAM_ID" --password "$PDL_APPLE_PASSWORD"
  # Sign every Mach-O first, including both Bluetooth helpers, then seal the app.
  while IFS= read -r -d '' binary; do
    if file -b "$binary" | grep -q 'Mach-O'; then
      codesign --force --sign "$PDL_SIGNING_IDENTITY" --keychain "$keychain" --timestamp --options runtime "$binary"
    fi
  done < <(find "$app/Contents" -type f -print0)
  codesign --force --sign "$PDL_SIGNING_IDENTITY" --keychain "$keychain" --timestamp --options runtime "$app"
  codesign --verify --deep --strict "$app"
  ditto -c -k --keepParent "$app" "$work/notarize.zip"
  xcrun notarytool submit "$work/notarize.zip" --keychain-profile pdl-release --keychain "$keychain" --wait
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose "$app"
fi
mkdir "$work/image"
ditto "$app" "$work/image/Personal Device Link.app"
ln -s /Applications "$work/image/Applications"
cp LICENSE "$work/image/LICENSE.txt"
if [ "$mode" = test ]; then
  printf '%s\n' 'TEST BUILD: ad-hoc signed, not Apple notarized. Gatekeeper may block it.' \
    '测试包：尚未通过 Apple 开发者签名和公证；macOS 可能拦截。请阅读仓库 docs/macos-quickstart.md。' > "$work/image/TEST-BUILD.txt"
fi
hdiutil create -volname 'Personal Device Link' -srcfolder "$work/image" -format UDZO -ov "$output"
if [ "$mode" = signed ]; then
  codesign --force --sign "$PDL_SIGNING_IDENTITY" --keychain "$keychain" --timestamp "$output"
  xcrun notarytool submit "$output" --keychain-profile pdl-release --keychain "$keychain" --wait
  xcrun stapler staple "$output"
  xcrun stapler validate "$output"
fi
(cd dist && shasum -a 256 "$(basename "$output")" > "$(basename "$output").sha256")
echo "Created: $output"
