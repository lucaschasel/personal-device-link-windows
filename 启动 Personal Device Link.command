#!/bin/bash
# Developer convenience only; downloaded scripts may be blocked by Gatekeeper.
# End users should install the signed/notarized app. See docs/macos-quickstart.md.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

on_exit() {
  result=$?
  if [ "$result" -ne 0 ]; then
    echo ''
    echo '未能完成启动。请查看上面的错误；按回车保留当前源码并退出。'
    read -r _ || true
  fi
}
trap on_exit EXIT

if [ "$(uname -s)" != Darwin ]; then
  echo '这个启动入口仅用于 Mac。Windows 请使用 Windows 程序包。'
  exit 1
fi

app="$PWD/desktop/src-tauri/target/release/bundle/macos/Personal Device Link.app"
if [ ! -d "$app" ] || [ ! -f .pdl-macos-build-complete ]; then
  echo '首次启动这个版本，正在自动构建。可能需要联网下载依赖，请保持窗口打开。'
  bash scripts/build-macos-gui.sh
  echo ''
  echo '已构建完成。Finder 将显示应用，可将它拖到“应用程序”，以后直接双击使用。'
  open -R "$app"
else
  echo '正在打开已构建的应用，本次不会重新编译。'
fi
open "$app"
