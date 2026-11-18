# 水印去除

本地工具，用逆混合去掉已知的半透明角标。它不上传图片，也不做生成式修复。

只处理你本人生成或已获得处理权的图片。不要用它去除他人作品上的权利标识。导出图会改写水印区域的像素，不会带走原图的内容凭证。能否达到像素级还原，以该模板的质量标记和每张图的验证报告为准。

`resources/` 里的 `synthetic_v1` 是合成夹具，不是豆包或千问的真实水印。真实模板要用你自己的黑图和中灰图标定，标定不过就保持 `quality: visual`。

## 环境

Rust 1.85 或更新（命令行依赖需要 edition 2024）。本仓库在 Rust 1.100 上测过。前端是 Node 上的 Vite。

```bash
cargo test --workspace
cargo run -p watermark-cli -- fixture --root .
cargo run -p watermark-cli -- process \
  --input resources/samples/synthetic.png \
  --template resources/templates/synthetic_v1.json \
  --out out
cargo run -p watermark-cli -- verify \
  --template resources/templates/synthetic_v1.json \
  --samples resources/samples/marked \
  --expect-clean resources/samples/clean
cargo run -p watermark-cli -- serve
npm install
npm run dev
```

浏览器打开 `http://127.0.0.1:1420`。`/api` 会转到 `127.0.0.1:8731`。桌面壳在 `src-tauri/`，不在 Cargo workspace 里，避免核心测试依赖 GTK：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

## 命令

- `calibrate --id --name --black --gray --out`：估计 Alpha 和色彩空间。灰图门禁不过时退出码 2。
- `verify --template --samples --expect-clean [--promote]`：只在有干净原图的留出集上计算 PSNR / SSIM。通过率低于 95% 时退出码 2。
- `process --input --template 或 --template-dir --out [--jpeg] [--accept] [--bbox x,y,w,h] [--no-watermark] [--keep-exif]`：原文件不覆盖。输出是 `<stem>.restored.png` 和 `<stem>.restored.report.json`。多张图时另写 `summary.csv`。
- `fixture`：重写合成模板和样张。
- `serve`：本地预览接口，默认 `127.0.0.1:8731`。

用户图片的报告没有 PSNR。JPEG 带 `lossy_output`，判决不会是 `pass`。
