#!/usr/bin/env bash
# 在打安装包之前裁剪已编译的桌面程序，避免把调试符号打进安装包。
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
triple="${TAURI_ENV_TARGET_TRIPLE:-}"
host=$(rustc -vV | awk '/^host:/{print $2}')

if [[ -n "$triple" && "$triple" != "$host" ]]; then
  dir="$root/src-tauri/target/${triple}/release"
else
  dir="$root/src-tauri/target/release"
fi

bin="$dir/watermark-remover"
if [[ ! -f "$bin" && -f "${bin}.exe" ]]; then
  bin="${bin}.exe"
fi
if [[ ! -f "$bin" ]]; then
  echo "找不到要裁剪的桌面程序：$bin" >&2
  exit 1
fi

strip_binary() {
  local file=$1
  local kind
  kind=$(file -b "$file")
  if [[ "$kind" == *PE32* ]]; then
    if ! command -v x86_64-w64-mingw32-strip >/dev/null 2>&1; then
      echo "没有 x86_64-w64-mingw32-strip，跳过 $file"
      return 0
    fi
    x86_64-w64-mingw32-strip --strip-unneeded "$file"
  else
    strip --strip-unneeded "$file"
  fi
}

strip_binary "$bin"
