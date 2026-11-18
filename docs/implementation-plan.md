# AI 水印去除客户端实际落地方案

本文把 [`document-after.md`](document-after.md) 收成可以按阶段实现和验收的工程方案。公式、阈值、数据结构和退出条件以可编码为准。原稿中的示意 JSON、未定义的置信度 0.85，以及按周排期，不直接作为实现合同。

合规边界保持不变：只处理用户本人生成或已有合法处理权的图片。本工具不提供绕过平台检测、批量洗白第三方图片或伪造内容凭证的能力。

---

## 1. 已纳入本方案的修正

| 议题 | 落地决定 | 依据 |
|---|---|---|
| 运行时 PSNR / SSIM | 用户图片没有干净原图。运行时只做框外逐字节一致性、重建残差和不稳定像素统计。PSNR、SSIM、最大像素差只在带真值的合成留出集上当验收门禁 | 原稿要求每张输出都报 PSNR > 50 dB。没有参照图时该数无法计算，硬算会把「公式自洽」误当成「还原了原图」 |
| 白色水印的混合模式 | `source-over` 与 Screen 在水印色为纯白时是同一公式，两套函数都保留。灰色样张用来确认「向白色插值」这一模型，不能用来区分这两种模式 | 穷举 256 个源值和 100 档 α，`C_wm = 1` 时两式差值为浮点误差（约 2e-16）。`C_wm = 0.5` 时最大差 0.5 |
| 不可逆阈值 `α ≥ 0.999` | 数值保护阈值仍是 `1 - α·C_wm < 1e-3`。像素级门禁另用 `alphaStableMax = 0.80`：稳定像素必须 MaxAbs ≤ 2，超过 0.80 的像素计入不稳定集合 | 同一色彩空间、8-bit 四舍五入往返：α=0.80 时最大误差 2；α=0.85 时最大误差 3，256 个源值里 63 个超过 2。等到 0.999 才标记，门禁已经失败 |
| 色彩空间 | 模板必须写明 `srgb` 或 `linear`。P0 用黑图和中灰图二选一，选错空间时误差是几十到上百个色阶，后续模块全部停 | 线性光混合后却在 sRGB 里求逆：α=0.3 / 0.5 / 0.7 的最大误差为 104 / 121 / 132，均值约 30–44 |
| Alpha 贴图位深 | 使用 16-bit 灰度 PNG。8-bit α 差 1/255，在 α=0.50 时最大像素误差已到 3，α=0.70 时到 5 | 正向用真值 α、逆向用量化 α 的穷举结果 |
| 定位置信度 | 不对 Alpha 图和原始 RGB 做互相关。用候选框外围像素做拉普拉斯填充，得到独立背景，再反推 α，与模板 Alpha 做 NCC。0.85 只是初始阈值，由正样本和位移负样本标定 | 用同一 α 先逆再正，残差衡量的是可逆性和舍入，错误模板也会得到很高的重建 PSNR |
| 框外零改动 | 输出缓冲从方向校正后的 8-bit 图克隆，只改写膨胀后的掩膜像素。禁止整图走一遍 f32 再量化 | 整图往返会让框外像素出现 ±1 |
| JPEG | 默认只出 PNG。框外一致性指解码并校正方向后的栅格。JPEG 重编码会改写跨过掩膜的 8×8 块，不能承诺文件级或块级一致 | 原稿同时要求默认 PNG 和「非水印区域不做重编码」。二者只在栅格、PNG 输出上同时成立 |
| 相对框和拉伸 | 位置用最短边的比例、固定宽高比、均匀缩放和亚像素平移。不用非均匀 `stretch-to-bbox` | 按宽、高分别归一会随图像宽高比改变水印外形 |
| 右下角连通域搜索 | v1 不做自动启发式分割，也不在未命中时自动修复。未命中则手动指定模板和位置；没有 Alpha 图就不改像素 | 低对比文字连通域会把签名、字幕当成水印 |
| LaMa / MI-GAN | v1 不接入 ONNX，不打包模型。不透明笔画的信息已经丢失，修复结果不能叫像素级还原。接口留到 v1.1，报告里必须标成非数学还原 | 安装包 < 30 MB、内存 < 1.5 GB 与 FP32 模型冲突。架构图里的 MI-GAN 在后文没有定义 |
| `rayon` / `ndarray` | v1 核心用平面切片。水印区域很小，耗时在解码和 PNG 编码 | 为小区域上并行运行时会增加依赖，对 2 秒预算没有帮助 |
| 模板热更新 | 只加载内置目录和用户本地目录。校验 JSON schema 与 Alpha 文件的 SHA-256。v1 不做远程自动拉取 | 未签名的远程模板可以替换还原公式的输入 |
| 内容凭证 | 不覆盖原文件。导出图不拷贝 C2PA 等内容凭证，报告写明像素已被修改、凭证未转移 | 把原凭证贴到改过的图上会伪造来源 |
| 排期 | 按依赖关系分阶段，每阶段有退出条件。不用周数当计划 | 日历工期不能代替可验证的出口 |

---

## 2. 目标与验收口径

### 2.1 产品目标

本地桌面程序，去掉豆包、千问等已知模板的右下角半透明水印。默认路径是混合公式的逆运算。每张图给出机器可读的验证报告。新增品牌时增加模板文件，不改逆运算代码。

### 2.2 两套验收，不能混用

**合成留出集（有干净原图）**，用于发布模板和发版：

- 评价区域是「α > 0.02 的像素」按切比雪夫距离外扩 `paddingPx`（默认 4）
- 该区域之外，与方向校正后的原图像素逐字节相同
- 在 `α ≤ alphaStableMax`（默认 0.80）的像素上：MaxAbs ≤ 2，PSNR > 50 dB
- 评价区域短边 ≥ 64 px 时，RGB 三通道 SSIM 的最小值 > 0.999
- 不稳定像素（α > 0.80 或触发数值保护）占核心像素的比例 ≤ 2%
- 被夹紧到 0 或 1 的像素数为 0
- 每个品牌 95% 的留出图同时满足以上条件，模板才能标成 `quality: pixel`

**用户图片（没有干净原图）**，运行时报告：

- 框外差异计数必须为 0，否则判失败并丢弃输出
- 独立背景反推的 α 与模板的 NCC、平均绝对误差
- 不稳定像素数、夹紧像素数、使用的模板版本
- 判决只使用这些可观测项，不打印伪造的 PSNR

留出集达不到 `pixel` 时，模板标成 `quality: visual`。界面使用「已按模板逆运算」，不使用「像素级还原」。这是 P0 的可能结论，不是实现失败后的文案修饰。

### 2.3 明确不做

- 任意未知水印的一键去除
- 把修复模型的输出称作像素级还原
- 重度压缩、二次截图、水印被裁切后的像素级承诺
- 覆盖用户原文件
- v1 捆绑或下载 LaMa、MI-GAN

---

## 3. 数学合同

通道独立，先把 8-bit 像素换成 `[0, 1]` 的 `f32`。`α ∈ [0, 1)`，水印色 `C_wm ∈ [0, 1]^3`。v1 默认 `C_wm = (1, 1, 1)`。

### 3.1 公式

标准覆盖（source-over）：

```text
C_out = α · C_wm + (1 - α) · C_src
C_src = (C_out - α · C_wm) / (1 - α)
```

Screen：

```text
k = α · C_wm
C_out = 1 - (1 - C_src) · (1 - k)
C_src = (C_out - k) / (1 - k)
```

`C_wm = 1` 时 `k = α`，Screen 化成 `C_out = α + (1 - α) · C_src`，与 source-over 相同。代码仍按 `blendMode` 分派，禁止在 Screen 分支调用 source-over 函数。单元测试锁定：纯白时两式差小于 `1e-6`，`C_wm = 0.5` 时存在至少一组输入使结果差大于 `0.1`。

### 3.2 舍入与数值保护

- 中间量保持 f32
- 回到 8-bit 时使用「一半远离 0」的舍入，与 Rust `f32::round` 一致，再夹到 0–255
- `1 - α · C_wm < 1e-3` 的像素不求逆，记入 `numeric_unstable`，输出保留观测值
- `α > alphaStableMax` 的像素仍然求逆，但记入 `quantization_unstable`，不参与 MaxAbs ≤ 2 的统计
- 求逆结果在夹紧之前先统计落在 `[0, 1]` 外的像素。夹紧数不为 0 时，该图不能判为通过

必须写进单测的向量：

| 用例 | 输入 | 期望 |
|---|---|---|
| 纯白等价 | `C_src=0.2, α=0.4, C_wm=1` | 两式 `C_out=0.52`，逆运算回到 0.2 |
| Screen 非纯白 | `C_src=0.2, α=0.4, C_wm=0.5` | Screen 的 `C_out=0.36`，逆运算回到 0.2。用 source-over 逆这个 0.36 得到约 0.2667，测试要断言这个错误值，防止公式被接错 |
| 8-bit 一半 | `src=100, α=0.5, C_wm=1` | 正向 `177.5` 舍入为 178；逆运算后舍入为 101；绝对误差 1 |
| 稳定边界 | `C_wm=1`，α 从 0.20 到 0.80，256 个源值 | 最大绝对误差 ≤ 2 |
| 不稳定边界 | `α=0.85, C_wm=1` | 最大绝对误差为 3，且超过 2 的源值不少于 60 个 |

同一色彩空间、`C_wm = 1`、一半远离 0 的穷举记录（实现后应用测试复算，偏差即失败）：

| α | 最大绝对误差 | 平均绝对误差 | 误差 > 2 的源值数 |
|---|---:|---:|---:|
| 0.20 | 1 | 0.199 | 0 |
| 0.40 | 1 | 0.398 | 0 |
| 0.50 | 1 | 0.500 | 0 |
| 0.60 | 1 | 0.598 | 0 |
| 0.70 | 2 | 0.801 | 0 |
| 0.75 | 2 | 0.996 | 0 |
| 0.80 | 2 | 1.195 | 0 |
| 0.85 | 3 | 1.641 | 63 |
| 0.90 | 5 | 2.500 | 128 |
| 0.95 | 10 | 4.941 | 190 |

### 3.3 色彩空间

`srgb`：在编码后的 0–1 数值上直接做第 3.1 节的运算。

`linear`：先按 IEC 61966-2-1 解码到线性光，做逆运算，再编码回 sRGB，最后舍入。线性路径即使公式正确，8-bit 量化后 MaxAbs ≤ 2 也经常不成立（α=0.30 时穷举最大误差 9，24 个源值超过 2）。P0 若判定平台在线性光混合，模板的 `alphaStableMax` 和对外文案改用实测包络，不沿用 0.80 和「≤ 2」。

选错空间的损失足够大，因此色彩空间是模板字段，不是运行时开关。

---

## 4. 标定

标定是发布模板的前置条件。样本放在 `templates-src/`，目录被 Git 忽略。仓库只提交由测试代码生成的小尺寸合成图。

### 4.1 每个品牌要采集的样张

优先使用平台导出的 PNG。只有 JPEG 时，P0 必须单独给出「该品牌能否标成 pixel」的结论。

- 纯黑图：至少 4 档分辨率（覆盖 512、1024、2048 和平台常用长边），每档至少 3 张
- 中灰图：RGB `(128, 128, 128)`，分辨率与黑图对齐
- 纯白图：只做对照组。白底上向白色混合的结果仍接近白色，不能用来估计 α
- 自有照片：20 张留出，另 20 张用于合成回归。这些照片必须同时保留加水印前的文件

分辨率档之间把 Alpha 缩放到同一宽度后做 NCC。NCC < 0.98 说明字号 hinting 导致形状不同，按宽度分段各发一个模板，不把一张图非均匀拉到所有尺寸。

### 4.2 估计 α 和色彩空间

1. 把同尺寸黑图对齐到整数像素（搜索范围 ±8 px），对 Alpha 支持区域取逐像素中位数，抑制压缩噪声。
2. 候选 A：`α = C_black`（sRGB 数值）。用它预测灰图：`C_gray = α + (1-α)·(128/255)`。
3. 候选 B：`α = decode(C_black)`，在线性光里与 `decode(128/255)` 混合，再编码回 sRGB。
4. 选灰图 MAE 更小的候选，作为模板的 `colorSpace`。
5. 两者 MAE 都大于 2，或灰图最大误差大于 6：单 α、纯白模型不成立。停止该品牌的 pixel 模板，先检查是否为彩色水印、每通道 α 或强压缩。不要进入界面开发。
6. 通过后把 α 写成 16-bit 灰度 PNG，`0` 为全透明，`65535` 为不透明。SHA-256 写入模板 JSON。

平台水印若不是纯白，黑图只能给出 `α·C_wm` 的乘积。此时增加一张已知底色，联立求解 α 和 `C_wm`，并把 `blendMode` 设为实测模式。纯白场景把 `blendMode` 写成 `source_over`，Screen 测试仍然保留。

### 4.3 放置参数

以图像最短边 `m = min(width, height)` 为长度单位：

```text
watermark_width  = round(widthRatio  * m)
watermark_height = round(watermark_width / aspect)
right_gap        = round(marginX * m)
bottom_gap       = round(marginY * m)
origin_x         = image_width  - right_gap - watermark_width  + subpixelX
origin_y         = image_height - bottom_gap - watermark_height + subpixelY
```

`widthRatio`、`aspect`、`marginX`、`marginY` 来自黑图支持区域的统计中位数。`subpixelX/Y` 在 `[-0.5, 0.5]` 上以 0.1 为步长，选灰图预测 MAE 最小的位置。Alpha 图按目标宽高等比缩放，插值用 Lanczos3，不单独拉宽或拉高。

预测宽度小于 `minWidthPx`（默认 48）时，该模板不参与这张图的匹配。

### 4.4 发布前回归

用冻结的模板把水印合成到留出照片上，再逆运算，按第 2.2 节的 pixel 门禁统计。95% 通过才把 `quality` 写成 `pixel`，并附 `templates-src/<id>/calibration-report.json`（不进 Git 的原始样张除外，报告本身进发布包）。

真实平台导出图没有干净原图，只做定位命中率和目视抽查，不计入这 95%。

---

## 5. 运行时管线

```text
解码 → EXIF 方向烘焙 → 色彩/位深检查 → 模板放置与 ±4 px 搜索
    → 独立背景反推 α → NCC 判决 → 只改写掩膜内像素
    → 框外逐字节校验 → PNG + report.json
```

1. 解码 PNG 或 JPEG。动画、CMYK、索引色在 v1 直接拒绝。
2. 按 EXIF Orientation 烘焙像素，输出里该字段写 1。其余 EXIF 可原样拷贝。
3. 16-bit 输入降到 8-bit，报告加 `precision_reduced`，判决最高为 `needs_review`。
4. 灰度图复制成 RGB 再处理。RGBA 的 A 通道原样拷贝，不参与混合。
5. 存在非 sRGB 的 ICC 时，报告加 `non_srgb`。v1 不在任意 ICC 里做色彩管理，判决最高为 `needs_review`。
6. 加载通过 schema 和哈希校验的模板。对每个模板按第 4.3 节放框，并在 ±4 px 内取 NCC 最大的位置。
7. 用框外 4 px 环带作边界，在框内做拉普拉斯填充（Jacobi，最多 200 次）得到独立背景 `B`。对 `|C_wm - B| > 0.05` 的像素：

```text
α_obs = clamp((C_out - B) / (C_wm - B), 0, 1)
```

与模板 α 计算 NCC 和 MAE。
8. NCC ≥ 高阈值：自动逆运算。介于低阈值与高阈值之间：界面标出候选框，用户确认后才写文件。低于低阈值：不修改，判决 `unsupported`。
9. 输出图像从烘焙后的 8-bit 缓冲克隆。只替换评价掩膜内的 RGB。逆运算使用模板的 `colorSpace` 和 `blendMode`。
10. 比较掩膜外字节。不一致则删除输出图，返回 `failed`，报告保留差异计数。
11. 默认写 PNG（压缩等级取「速度优先」，以便 4K 预算用在编码上）和同名 `.report.json`。用户显式选择 JPEG 时，报告加 `lossy_output`，判决最高为 `needs_review`，且不承诺框外一致。

高阈值初值 0.85，低阈值初值 0.60。标定报告必须给出：真实平台图的 NCC 分布，以及把框平移 16 px、无水印图两类负样本的 NCC 分布。高阈值取到负样本零通过的最低值；若与正样本重叠，则关闭自动写文件，全部改为确认后处理。

---

## 6. 代码结构

逻辑放在库里，桌面壳和命令行都调用它。逆运算不写在 Tauri 命令函数中。

```text
watermark-remover/
├── crates/watermark-core/        # 模板、放置、逆运算、定位、报告
├── crates/watermark-cli/         # calibrate / verify / process
├── src-tauri/                    # IPC、文件对话框、合规状态
├── src/                          # React + TypeScript + Vite
├── resources/templates/          # 随包模板 JSON
├── resources/alpha_maps/         # 16-bit 灰度 PNG
├── templates-src/                # 本地样本与标定报告，样本不入库
└── docs/
```

`watermark-core` 对外保持这些类型。名称可以调整，职责不能合并成一个「处理一切」的函数。

```rust
enum BlendMode { SourceOver, Screen }
enum ColorSpace { Srgb, Linear }
enum Quality { Pixel, Visual }

struct Template { /* schemaVersion, id, version, blend, color, placement, hashes */ }

enum InversePixel { Value(f32), NumericUnstable }

fn inverse_pixel(out: f32, alpha: f32, wm: f32, mode: BlendMode) -> InversePixel;

struct Placement { origin_x: f32, origin_y: f32, width: u32, height: u32 }

fn place(template: &Template, image_w: u32, image_h: u32) -> Option<Placement>;

struct MatchScore { ncc: f32, alpha_mae: f32, shift: (i32, i32) }

fn locate(image: &RgbImage, template: &Template) -> MatchScore;

struct Report { verdict: Verdict, /* 见第 7 节 */ }

fn restore(image: &RgbaImage, template: &Template, shift: (i32, i32)) -> Result<(RgbaImage, Report), RestoreError>;
```

v1 依赖：`image`、`serde`、`serde_json`、CLI 用 `clap`、桌面用 Tauri 2 与 React。不引入 `ort`、`ndarray`、`rayon`。

---

## 7. 模板与报告

### 7.1 模板

未知字段拒绝加载。`alphaSha256` 与文件字节不一致时拒绝加载。

```json
{
  "schemaVersion": 1,
  "id": "doubao_v1",
  "displayName": "豆包 AI 水印 v1",
  "version": 1,
  "quality": "pixel",
  "blendMode": "source_over",
  "colorSpace": "srgb",
  "color": [1.0, 1.0, 1.0],
  "alphaMap": "alpha_maps/doubao_v1.png",
  "alphaDepth": 16,
  "alphaSha256": "<lowercase hex of file bytes>",
  "placement": {
    "anchor": "bottom-right",
    "sizeBasis": "min-side",
    "widthRatio": 0.18,
    "aspect": 3.2,
    "marginX": 0.02,
    "marginY": 0.02,
    "subpixel": [0.0, 0.0],
    "minWidthPx": 48,
    "paddingPx": 4
  },
  "alphaStableMax": 0.80,
  "minImageSize": [256, 256],
  "nccAuto": 0.85,
  "nccConfirm": 0.60
}
```

`widthRatio`、`aspect` 和边距在标定完成前不写进发布模板。原稿示例里的 `-0.22, -0.12` 只是占位，不能进资源目录。

同一 `id` 可以有多个 `version`。匹配时取得分最高且达到确认阈值的版本。两者得分差小于 0.03 时视为歧义，改为用户选择。

用户模板目录是应用配置目录下的 `templates/`。启动时加载，提供「重新加载」按钮。v1 没有网络更新。

### 7.2 单图报告

```json
{
  "schemaVersion": 1,
  "input": { "width": 2048, "height": 2048, "format": "png" },
  "output": { "format": "png", "lossy": false },
  "templateId": "doubao_v1",
  "templateVersion": 1,
  "qualityClaim": "pixel",
  "blendMode": "source_over",
  "colorSpace": "srgb",
  "bbox": [1600, 1880, 368, 115],
  "shift": [0, 1],
  "ncc": 0.93,
  "alphaMae": 0.04,
  "method": "inverse_blend",
  "verdict": "pass",
  "flags": [],
  "counts": {
    "corePixels": 4200,
    "unstablePixels": 30,
    "clampedPixels": 0,
    "outsideDiffPixels": 0
  },
  "holdout": null
}
```

`verdict` 取值：

| 值 | 条件 |
|---|---|
| `pass` | 模板 `quality` 为 pixel，NCC 达到自动阈值，框外差异 0，夹紧数 0，不稳定比例 ≤ 2%，且没有 `precision_reduced`、`non_srgb`、`lossy_output` |
| `pass_with_unstable` | 上式里只有不稳定比例超过 2%，其余满足。界面展示不稳定像素 |
| `needs_review` | 用户确认后处理的低分匹配、视觉质量模板、16-bit、非 sRGB 或有损导出 |
| `no_watermark` | 用户选择「此图无水印」，输出为输入的无损拷贝 |
| `unsupported` | 没有达到确认阈值的模板 |
| `failed` | 框外被改写、IO 错误或模板损坏。不保留图像输出 |

`holdout` 仅在 `verify` 子命令写入，字段为 `psnr`、`ssimMin`、`maxAbs`、`stableMaxAbs`。桌面处理路径把该字段保持为 `null`。

批量汇总是 CSV，列顺序固定：

```text
input,output,template_id,template_version,verdict,ncc,alpha_mae,unstable_pixels,clamped_pixels,outside_diff,flags
```

路径只出现在用户选定的输出目录里，不进日志。

---

## 8. 界面

桌面端是 Tauri 2。文件访问限定在用户选中的目录。

- 单张：拖入或选择文件，显示自动匹配结果。通过时左右对比，空格切换原图与结果，滚轮放大。探针同时给出处理前 RGB、处理后 RGB、模板 α、不稳定标记。
- 未达自动阈值：只画候选框和 NCC，不生成结果文件，直到用户确认或改选模板。
- 手动：用户选择某个已安装模板并拖动矩形。程序按该矩形均匀放入此模板的 Alpha 图，判决最高 `needs_review`。没有模板的自由蒙版只导出掩膜 PNG，不改原图像素。
- 批量：一次处理一张，默认并发 1。列表显示判决。结束时写汇总 CSV。失败项不中断队列。
- 报告面板：模板 id 与版本、NCC、不稳定像素数、框外差异、质量声明。`holdout` 为空时不显示 PSNR。
- 设置：用户模板目录、PNG/JPEG 导出、是否拷贝 EXIF。没有模型路径设置。
- 首次启动和每次导出都显示合规说明，用户确认后才继续。文案：

> 本工具只用于处理你本人生成或已获得处理权的图片。请不要用它去除他人作品上的权利标识。导出图会改写水印区域的像素，并不会带走原图的内容凭证。能否达到像素级还原，以该模板的质量标记和每张图的验证报告为准。

原文件始终保留。输出命名为 `<stem>.restored.png` 与 `<stem>.restored.report.json`。

---

## 9. v1 与后续修复的边界

v1 的失败路径是「不改像素 + 明确判决」，不是生成式填补。

`α` 接近 1 的笔画在数学上不可恢复。v1 保留这些像素的观测值并在图上标出。后续若增加修复后端，必须实现单独接口，例如 `fill_unstable(region, mask) -> pixels`，且报告 `method` 写成 `inpaint`，`qualityClaim` 写成 `visual`，判决最高 `needs_review`。LaMa 只作为该接口的一个实现，按需加载，不进默认安装包。MI-GAN 不在计划内，除非以后有单独的评估结论。

修复后端的掩膜取 `α > 0.02` 的像素再外扩 2 px。模型推理可以使用更大的上下文窗口，贴回时只写掩膜内像素，避免破坏框外一致性。

---

## 10. 测试

| 测试 | 断言 |
|---|---|
| `inverse_matches_forward_white` | 第 3.2 节纯白向量，两模式差 `< 1e-6` |
| `screen_differs_when_color_not_white` | `C_wm=0.5` 的向量，以及「接错公式会得到 0.2667」 |
| `round_half_away_from_zero` | `src=100, α=0.5` 得到 178，再逆运算得到 101 |
| `stable_alpha_exhaustive` | α ∈ {0.20, 0.40, 0.50, 0.60, 0.70, 0.75, 0.80} 的最大误差不超过表中值 |
| `unstable_alpha_is_flagged` | α=0.85 的像素进入不稳定集合，不把整图判成 pass |
| `outside_mask_unchanged` | 掩膜外字节差为 0；故意改一个框外像素时 `restore` 返回 `failed` |
| `exif_orientation_bottom_right` | Orientation 为 6 的图，水印落在视觉右下角而不是文件右下角 |
| `rgba_alpha_preserved` | 颜色通道按公式变化，A 通道不变 |
| `template_hash_mismatch` | 改一个 PNG 字节后模板加载失败 |
| `locate_shift_negative` | 平移 16 px 的框不得达到自动阈值（用合成夹具） |
| `holdout_gate` | `verify` 对合成夹具写入 psnr / ssim / maxAbs，并按 95% 规则给出退出码 |
| `jpeg_export_not_pixel` | 选择 JPEG 时判决不是 `pass`，且带 `lossy_output` |
| `batch_rss` | 100 张 4000×3000 依次处理，进程峰值 RSS < 1.5 GB，相对第 1 张结束后的增长 < 50 MB |
| `four_k_latency` | 同尺寸 PNG，热启动中位数 < 2 秒（含解码、处理、写 PNG 和报告） |

夹具由测试代码合成：已知 α 图、纯白 source-over、sRGB。不依赖平台账号，CI 可以跑全部单元测试和夹具集成测试。真实平台回归在本地用 `watermark-cli verify` 跑，样本不进 CI。

性能数字在 Phase E 用 release 构建测量。未测量前，不把「已经满足 2 秒 / 1.5 GB」写进发布说明。

---

## 11. 实施顺序

每一阶段的出口未满足时，不开始依赖它的下一阶段。

### Phase A — 公式与色彩空间

交付 `watermark-core` 的逆运算、舍入和单测，加上 `calibrate` 的黑/灰估计。

出口：第 3.2 节单测全部通过；每个目标品牌有一份本地标定备忘，写明 `colorSpace`、灰图 MAE，以及 `pixel` 或 `visual` 的判断。灰图 MAE 不满足第 4.2 节时，该品牌停在这一阶段。

### Phase B — 命令行闭环

交付模板加载、放置、±4 px 定位、掩膜内逆运算、框外校验、`process` 与 `verify`。

出口：合成留出集按第 2.2 节达到 95%，框外差异为 0。报告里的用户路径不出现 PSNR。

### Phase C — 两个品牌模板

交付豆包、千问各一个通过 Phase A/B 的模板包（JSON + 16-bit Alpha + 标定报告）。

出口：模板哈希校验通过，`quality` 与实测一致。形状随分辨率明显变化时，出口改成「每个必要的尺寸段一个版本」，而不是一张拉伸图。

### Phase D — 桌面单张

交付 Tauri 壳、对比与探针、报告面板、合规确认、PNG 导出。

出口：用 CLI 和桌面处理同一夹具，报告 JSON 除路径外一致。未达自动阈值时，磁盘上不出现结果图。原文件字节不变。

### Phase E — 批量、打包、异常样张

交付批量队列、CSV、三平台安装包（Windows MSI 或 NSIS、macOS DMG、Linux AppImage）。

出口：第 10 节的 RSS 与 4K 耗时测试有记录；安装包不含 ONNX，体积 < 30 MB；无水印图、裁切掉水印的图、二次截图分别得到 `no_watermark` 或 `unsupported`，而不是 `pass`。

### Phase F — 可选修复后端

不作为 v1 出口。只有在 Phase C 的模板已经发布、并且确实存在大块不可逆像素时才做。接口和标注要求见第 9 节。

---

## 12. 风险与对应动作

| 风险 | 动作 |
|---|---|
| 平台在线性光混合，≤2 的门禁达不到 | Phase A 把模板标成 `visual` 或改用实测包络。界面不再显示像素级承诺 |
| 平台只提供低质量 JPEG | 标定报告写明噪声水平。达不到门禁就保持 `visual`，不把阈值放宽到「能过为止」 |
| 水印改版 | 旧模板 NCC 下降后自动路径停止，用户看到 `unsupported`。新样本重走 Phase A–C，`version` 加 1 |
| 水印不是纯白 | 停用纯白公式，改走第 4.2 节的联立估计，并补 Screen 与 source-over 的灰图对照 |
| 8-bit 舍入在笔画中心超阈 | 这些像素进入不稳定集合。比例 ≤ 2% 仍可 `pass`，超过则 `pass_with_unstable` |
| 把修复结果当成数学还原 | v1 没有修复后端。以后有，也必须改 `method` 和 `qualityClaim` |
| 模板文件被替换 | SHA-256 不符则拒绝加载 |
| 凭证被误用 | 不覆盖原图，导出图不携带 C2PA |

---

## 13. v1 交付物

1. `watermark-cli` 与三平台桌面安装包，安装包不含模型，体积 < 30 MB。
2. 已标定的豆包、千问模板（JSON、16-bit Alpha PNG、标定报告）。未达到 pixel 门禁的模板必须标成 `visual`。
3. `watermark-core` 单测、合成夹具和 `verify` 的留出集结果。
4. 用户可见的合规确认，以及报告字段说明（写在应用内的报告面板，并在发版说明里保留一份）。
5. 性能记录：4K 热启动中位数、100 张峰值 RSS。

`docs/calibration.md` 和 `docs/acceptance.md` 在 Phase A、Phase E 有真实数字后再写。现在不预置空文档。
