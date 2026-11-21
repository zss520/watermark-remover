#!/usr/bin/env bash
# 打发布包。
#
#   npm run package
#   npm run package:windows
#
# 当前系统的产物在 dist/：
#   watermark-remover-<版本>-<target>.tar.gz
#     内含 watermark-cli、桌面程序和 resources/doubao
#   当前系统能打出来的安装包（Linux 为 deb，有工具时再加 AppImage；
#     macOS 为 dmg；Windows 为 nsis）
#
# package:windows 在 Linux 或 macOS 上交叉编译 Windows 安装包。
# 需要 cargo-xwin、makensis、llvm-rc，以及 rustup target add x86_64-pc-windows-msvc。
# 安装包和便携 zip 会再复制到 release/windows/。
#
# 安装包不含 ONNX。压缩包和安装包都要小于 30 MB。
#
# 每次重新打包前把修订号加一。要一起改 package.json、package-lock.json、
# src-tauri/tauri.conf.json、src-tauri/Cargo.toml、
# crates/watermark-cli/Cargo.toml、crates/watermark-core/Cargo.toml，
# 以及 Cargo.lock 和 src-tauri/Cargo.lock 里这两个包自己的版本。
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

mode="${1:-host}"
version=$(node -p "require('./package.json').version")
limit=$((30 * 1024 * 1024))
cross=0

if [[ "$mode" == "windows" || "$mode" == "x86_64-pc-windows-msvc" ]]; then
  cross=1
  target="x86_64-pc-windows-msvc"
  bundles=(nsis)
  if ! command -v cargo-xwin >/dev/null 2>&1; then
    echo "需要 cargo-xwin，才能在当前系统上编译 Windows 程序" >&2
    exit 1
  fi
  if ! command -v makensis >/dev/null 2>&1; then
    echo "需要 makensis（NSIS）才能制作 Windows 安装包" >&2
    exit 1
  fi
  if ! rustup target list --installed | grep -qx "$target"; then
    echo "需要先执行：rustup target add ${target}" >&2
    exit 1
  fi
  if ! command -v llvm-rc >/dev/null 2>&1; then
    for candidate in /usr/lib/llvm-18/bin /usr/lib/llvm-19/bin /usr/lib/llvm-17/bin; do
      if [[ -x "${candidate}/llvm-rc" ]]; then
        export PATH="${candidate}:${PATH}"
        break
      fi
    done
  fi
  if ! command -v llvm-rc >/dev/null 2>&1; then
    echo "需要 llvm-rc，才能给 Windows 程序嵌入图标" >&2
    exit 1
  fi
elif [[ "$mode" == "host" ]]; then
  target=$(rustc -vV | awk '/^host:/{print $2}')
  bundles=()
  os=$(uname -s)
  if [[ "$os" == "Linux" ]]; then
    if command -v dpkg-deb >/dev/null 2>&1; then
      bundles+=(deb)
    else
      echo "没有 dpkg-deb，跳过 deb"
    fi
    if command -v appimagetool >/dev/null 2>&1 || command -v linuxdeploy >/dev/null 2>&1; then
      bundles+=(appimage)
    else
      echo "没有 appimagetool，跳过 AppImage"
    fi
  elif [[ "$os" == "Darwin" ]]; then
    bundles+=(dmg)
  else
    bundles+=(nsis)
  fi
else
  echo "未知打包目标：$mode。可用 host 或 windows" >&2
  exit 1
fi

name="watermark-remover-${version}-${target}"
stage="dist/${name}"
if (( cross )); then
  archive="dist/${name}.zip"
  bundle_dir="src-tauri/target/${target}/release/bundle"
else
  archive="dist/${name}.tar.gz"
  bundle_dir="src-tauri/target/release/bundle"
fi

strip_binary() {
  local file=$1
  local kind
  kind=$(file -b "$file")
  if [[ "$kind" == *PE32* ]]; then
    if command -v x86_64-w64-mingw32-strip >/dev/null 2>&1; then
      x86_64-w64-mingw32-strip --strip-unneeded "$file" || true
    fi
  elif command -v strip >/dev/null 2>&1; then
    strip --strip-unneeded "$file" || true
  fi
}

echo "编译 watermark-cli（${target}）"
if (( cross )); then
  cargo xwin build --release --target "$target" -p watermark-cli
  cli="target/${target}/release/watermark-cli.exe"
else
  cargo build --release -p watermark-cli
  cli="target/release/watermark-cli"
  if [[ ! -f "$cli" && -f "${cli}.exe" ]]; then
    cli="${cli}.exe"
  fi
fi
if [[ ! -f "$cli" ]]; then
  echo "没有找到命令行程序：$cli" >&2
  exit 1
fi
strip_binary "$cli"

if (( ${#bundles[@]} > 0 )); then
  joined=$(IFS=,; echo "${bundles[*]}")
  echo "制作安装包：${joined}"
  rm -rf "$bundle_dir"
  if (( cross )); then
    npx --yes @tauri-apps/cli@2.12.6 build --runner cargo-xwin --target "$target" --bundles "$joined"
    app="src-tauri/target/${target}/release/watermark-remover.exe"
  else
    npx --yes @tauri-apps/cli@2.12.6 build --bundles "$joined"
    app="src-tauri/target/release/watermark-remover"
    if [[ ! -f "$app" && -f "${app}.exe" ]]; then
      app="${app}.exe"
    fi
  fi
else
  echo "编译前端和桌面程序"
  npm run build
  cargo build --release --manifest-path src-tauri/Cargo.toml
  app="src-tauri/target/release/watermark-remover"
  if [[ ! -f "$app" && -f "${app}.exe" ]]; then
    app="${app}.exe"
  fi
fi

if [[ ! -f "$app" ]]; then
  echo "没有找到桌面程序：$app" >&2
  exit 1
fi

rm -rf "$stage" "$archive"
mkdir -p "$stage/resources"
cp "$cli" "$app" "$stage/"
cp -a resources/doubao "$stage/resources/doubao"

cli_name=$(basename "$cli")
app_name=$(basename "$app")
if (( cross )); then
  cat > "$stage/使用说明.txt" << EOF
水印去除 ${version}（Windows）

安装包文件名是「水印去除_${version}_x64-setup.exe」。双击安装后，从开始菜单打开「水印去除」。
安装程序使用简体中文，默认装到当前用户目录，不需要管理员权限。
如果系统里还没有 WebView2，安装程序会下载微软的 WebView2 运行库，这一步需要联网。

也可以不安装，直接在这个目录里运行。不要把程序和 resources 拆开：

  ${app_name}
  ${cli_name} serve --bind 127.0.0.1:8731

豆包模板在 resources\\doubao。去水印和修改相机时间可以分开使用。
命令行只改相机时间、不去水印：

  ${cli_name} process --input 图片.jpg --out 输出 --no-watermark --camera-time 2026-12-01T09:30

两个一起做：

  ${cli_name} process --input 图片.jpg --template-dir resources\\doubao --out 输出 --camera-time 2026-12-01T09:30
EOF
else
  cat > "$stage/使用说明.txt" << EOF
水印去除 ${version}

在这个目录里运行，不要把三个部分拆开：

  ./${app_name}
  ./${cli_name} serve --bind 127.0.0.1:8731

豆包模板在 resources/doubao。去水印和修改相机时间可以分开使用。
命令行只改相机时间、不去水印：

  ./${cli_name} process --input 图片.jpg --out 输出 --no-watermark --camera-time 2026-12-01T09:30

两个一起做：

  ./${cli_name} process --input 图片.jpg --template-dir resources/doubao --out 输出 --camera-time 2026-12-01T09:30
EOF
fi

if find "$stage" -iname '*.onnx' -print | grep -q .; then
  echo "发布目录里有 ONNX 模型，已停止" >&2
  exit 1
fi
if [[ ! -f "$stage/resources/doubao/template_1024x1024.json" ]]; then
  echo "发布目录里没有豆包模板" >&2
  exit 1
fi

if (( cross )); then
  if ! command -v x86_64-w64-mingw32-objdump >/dev/null 2>&1; then
    echo "需要 x86_64-w64-mingw32-objdump 来检查 Windows 程序依赖" >&2
    exit 1
  fi
  for file in "$stage/$cli_name" "$stage/$app_name"; do
    imports=$(x86_64-w64-mingw32-objdump -p "$file")
    if grep -q 'VCRUNTIME140.dll' <<<"$imports"; then
      echo "${file} 依赖 VCRUNTIME140.dll，安装包不能带上这个运行库要求" >&2
      exit 1
    fi
  done
  if ! grep -q 'Subsystem.*Windows GUI' <<<"$(x86_64-w64-mingw32-objdump -p "$stage/$app_name")"; then
    echo "桌面程序不是 Windows 图形子系统" >&2
    exit 1
  fi
fi

if (( cross )); then
  python3 - "$stage" "$archive" << 'PY'
import sys
import zipfile
from pathlib import Path

stage = Path(sys.argv[1])
archive = Path(sys.argv[2])
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
    for path in sorted(stage.rglob("*")):
        if path.is_file():
            zipped.write(path, Path(stage.name) / path.relative_to(stage))
PY
else
  tar -C dist -czf "$archive" "$name"
fi

installers=()
if [[ -d "$bundle_dir" ]]; then
  while IFS= read -r file; do
    base=$(basename "$file")
    cp "$file" "dist/${base}"
    installers+=("dist/${base}")
  done < <(find "$bundle_dir" -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*.dmg' -o -name '*.msi' -o -name '*.exe' \))
fi
if (( ${#bundles[@]} > 0 && ${#installers[@]} == 0 )); then
  echo "安装包没有生成" >&2
  exit 1
fi

if (( cross == 0 )); then
  echo "检查打包后的模板查找"
  probe_port=8741
  probe_log=$(mktemp)
  (cd /tmp && "$root/$stage/$cli_name" serve --bind "127.0.0.1:${probe_port}") >"$probe_log" 2>&1 &
  probe_pid=$!
  ok=0
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
    if curl -sf "http://127.0.0.1:${probe_port}/api/templates" | grep -q '豆包'; then
      ok=1
      break
    fi
    sleep 0.2
  done
  kill "$probe_pid" >/dev/null 2>&1 || true
  wait "$probe_pid" 2>/dev/null || true
  if (( ok == 0 )); then
    echo "打包后的 CLI 没有找到豆包模板" >&2
    cat "$probe_log" >&2 || true
    rm -f "$probe_log"
    exit 1
  fi
  rm -f "$probe_log"
fi

verify_nsis() {
  local file=$1
  local listing
  listing=$(mktemp)
  if ! 7z l -ba "$file" >"$listing" 2>/dev/null; then
    echo "无法列出安装包内容：$file" >&2
    rm -f "$listing"
    exit 1
  fi
  if ! grep -q 'template_1024x1024.json' "$listing"; then
    echo "安装包里没有豆包模板" >&2
    cat "$listing" >&2
    rm -f "$listing"
    exit 1
  fi
  if grep -qi '\.onnx' "$listing"; then
    echo "安装包里有 ONNX 模型" >&2
    rm -f "$listing"
    exit 1
  fi
  rm -f "$listing"
}

echo "发布文件："
for file in "$archive" "${installers[@]+"${installers[@]}"}"; do
  file_bytes=$(wc -c < "$file" | tr -d ' ')
  if (( file_bytes > limit )); then
    echo "${file} 有 ${file_bytes} 字节，超过 30 MB" >&2
    exit 1
  fi
  if [[ "$file" == *.deb ]]; then
    listing=$(mktemp)
    dpkg-deb -c "$file" > "$listing"
    if ! grep -q 'template_1024x1024.json' "$listing"; then
      echo "deb 里没有豆包模板" >&2
      rm -f "$listing"
      exit 1
    fi
    if grep -qi '\.onnx' "$listing"; then
      echo "deb 里有 ONNX 模型" >&2
      rm -f "$listing"
      exit 1
    fi
    rm -f "$listing"
  fi
  if [[ "$file" == *.exe ]]; then
    verify_nsis "$file"
  fi
  if [[ "$file" == *.zip ]]; then
    listing=$(mktemp)
    unzip -l "$file" > "$listing"
    if ! grep -q 'template_1024x1024.json' "$listing"; then
      echo "zip 里没有豆包模板" >&2
      rm -f "$listing"
      exit 1
    fi
    if grep -qi '\.onnx' "$listing"; then
      echo "zip 里有 ONNX 模型" >&2
      rm -f "$listing"
      exit 1
    fi
    rm -f "$listing"
  fi
  kind=$(file -b "$file")
  if [[ "$file" == *.exe && "$kind" != *PE32* && "$kind" != *NSIS* ]]; then
    echo "安装包不是 Windows 可执行文件：$kind" >&2
    exit 1
  fi
  printf '  %s  %s 字节\n' "$file" "$file_bytes"
done

if (( cross )); then
  release_dir="release/windows"
  mkdir -p "$release_dir"
  find "$release_dir" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
  cp "$archive" "$release_dir/"
  cp "$stage/使用说明.txt" "$release_dir/使用说明.txt"
  for file in "${installers[@]+"${installers[@]}"}"; do
    cp "$file" "$release_dir/"
  done
  (
    cd "$release_dir"
    mapfile -t files < <(find . -maxdepth 1 -type f ! -name '校验.txt' -printf '%P\n' | LC_ALL=C sort)
    sha256sum -- "${files[@]}" > 校验.txt
  )
  echo "已复制到 ${release_dir}/"
fi
