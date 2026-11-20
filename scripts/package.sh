#!/usr/bin/env bash
# 打当前系统的发布包。
#
#   npm run package
#
# 产物在 dist/：
#   watermark-remover-<版本>-<target>.tar.gz
#     内含 watermark-cli、桌面程序和 resources/doubao
#   当前系统能打出来的安装包（Linux 为 deb，有工具时再加 AppImage；
#     macOS 为 dmg；Windows 为 nsis）
#
# 安装包不含 ONNX。压缩包和安装包都要小于 30 MB。
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(node -p "require('./package.json').version")
target=$(rustc -vV | awk '/^host:/{print $2}')
name="watermark-remover-${version}-${target}"
stage="dist/${name}"
archive="dist/${name}.tar.gz"
limit=$((30 * 1024 * 1024))

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

echo "编译 watermark-cli"
cargo build --release -p watermark-cli

if (( ${#bundles[@]} > 0 )); then
  joined=$(IFS=,; echo "${bundles[*]}")
  echo "制作安装包：${joined}"
  rm -rf src-tauri/target/release/bundle
  npx --yes @tauri-apps/cli@2.12.6 build --bundles "$joined"
else
  echo "编译前端和桌面程序"
  npm run build
  cargo build --release --manifest-path src-tauri/Cargo.toml
fi

cli="target/release/watermark-cli"
app="src-tauri/target/release/watermark-remover"
if [[ ! -f "$cli" && -f "${cli}.exe" ]]; then
  cli="${cli}.exe"
fi
if [[ ! -f "$app" && -f "${app}.exe" ]]; then
  app="${app}.exe"
fi
if [[ ! -f "$cli" || ! -f "$app" ]]; then
  echo "没有找到编译产物：$cli 或 $app" >&2
  exit 1
fi

rm -rf "$stage" "$archive"
mkdir -p "$stage/resources"
cp "$cli" "$app" "$stage/"
cp -a resources/doubao "$stage/resources/doubao"
if command -v strip >/dev/null 2>&1; then
  strip --strip-unneeded "$stage/$(basename "$cli")" "$stage/$(basename "$app")" || true
fi
cat > "$stage/使用说明.txt" << EOF
水印去除 ${version}

在这个目录里运行，不要把三个部分拆开：

  ./$(basename "$app")
  ./$(basename "$cli") serve --bind 127.0.0.1:8731

豆包模板在 resources/doubao。命令行改相机时间：

  ./$(basename "$cli") process --input 图片.jpg --template-dir resources/doubao --out 输出 --camera-time 2026-12-01T09:30
EOF

if find "$stage" -iname '*.onnx' -print | grep -q .; then
  echo "发布目录里有 ONNX 模型，已停止" >&2
  exit 1
fi

tar -C dist -czf "$archive" "$name"

installers=()
if [[ -d src-tauri/target/release/bundle ]]; then
  while IFS= read -r file; do
    base=$(basename "$file")
    cp "$file" "dist/${base}"
    installers+=("dist/${base}")
  done < <(find src-tauri/target/release/bundle -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*.dmg' -o -name '*.msi' -o -name '*.exe' \))
fi
if (( ${#bundles[@]} > 0 && ${#installers[@]} == 0 )); then
  echo "安装包没有生成" >&2
  exit 1
fi

echo "检查打包后的模板查找"
probe_port=8741
probe_log=$(mktemp)
(cd /tmp && "$root/$stage/$(basename "$cli")" serve --bind "127.0.0.1:${probe_port}") >"$probe_log" 2>&1 &
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
  printf '  %s  %s 字节\n' "$file" "$file_bytes"
done
