use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use watermark_core::{
    calibrate_to_dir, decode_bytes, decode_path, load_template_dir, load_template_embedded_parts, load_template_file,
    process_best, read_template_files, verify_pairs, CalibrateRequest, InputImage, LoadedTemplate, ProcessOptions,
    ProcessOutput, Verdict, VerifyReport,
};

#[derive(Parser)]
#[command(name = "watermark-cli", about = "已知半透明水印的逆混合还原")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 用纯黑图和中灰图估计 Alpha、色彩空间和放置参数
    Calibrate(CalibrateArgs),
    /// 用带干净原图的留出集检查像素级门禁
    Verify(VerifyArgs),
    /// 处理一张图或一个目录
    Process(ProcessArgs),
    /// 把合成示例模板写到 resources/，供界面和回归使用
    Fixture(FixtureArgs),
    /// 本地预览服务，供桌面壳和浏览器使用同一套核心
    Serve(ServeArgs),
}

#[derive(Args)]
struct CalibrateArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    name: String,
    #[arg(long = "black")]
    black: Vec<PathBuf>,
    #[arg(long = "gray")]
    gray: Vec<PathBuf>,
    /// 第三张平涂，RGB 192。用来拟合带描边的水印
    #[arg(long = "light")]
    light: Vec<PathBuf>,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct VerifyArgs {
    #[arg(long)]
    template: PathBuf,
    #[arg(long)]
    samples: PathBuf,
    #[arg(long)]
    expect_clean: PathBuf,
    /// 门禁通过时把模板 quality 写成 pixel
    #[arg(long)]
    promote: bool,
}

#[derive(Args)]
struct ProcessArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    template: Option<PathBuf>,
    #[arg(long)]
    template_dir: Option<PathBuf>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    jpeg: bool,
    /// 置信度处于确认区间时仍然写出结果，判决最高为 needs_review
    #[arg(long)]
    accept: bool,
    #[arg(long)]
    bbox: Option<String>,
    /// 不去除水印。可以单独和 --camera-time 一起用，只改相机时间。
    #[arg(long)]
    no_watermark: bool,
    #[arg(long)]
    keep_exif: bool,
    /// 改写相机时间，例如 2026-12-01T09:30。原图没有秒时不补秒。
    #[arg(long)]
    camera_time: Option<String>,
}

#[derive(Args)]
struct FixtureArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:8731")]
    bind: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("错误：{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().cmd {
        Cmd::Calibrate(args) => cmd_calibrate(args),
        Cmd::Verify(args) => cmd_verify(args),
        Cmd::Process(args) => cmd_process(args),
        Cmd::Fixture(args) => {
            watermark_core::fixture::write_resources(&args.root)?;
            println!("已写入 {}", args.root.join("resources").display());
            Ok(())
        }
        Cmd::Serve(args) => serve(&args.bind),
    }
}

fn cmd_calibrate(args: CalibrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let blacks = load_images(&args.black)?;
    let grays = load_images(&args.gray)?;
    let lights = load_images(&args.light)?;
    let summary = calibrate_to_dir(
        &CalibrateRequest {
            id: args.id,
            display_name: args.name,
            blacks,
            grays,
            lights,
        },
        &args.out,
    )?;
    if summary.affine {
        println!(
            "仿射 sRGB，浅灰残差 MAE {:.3}，最大误差 {}，模板 {} 个，质量保持视觉级",
            summary.gray_mae, summary.gray_max, summary.template_count
        );
    } else {
        println!(
            "色彩空间 {}，灰图 MAE {:.3}，最大误差 {}，模板 {} 个",
            summary.color_space, summary.gray_mae, summary.gray_max, summary.template_count
        );
    }
    if !summary.model_ok {
        eprintln!("模型未通过灰图门禁，未作为 pixel 模板发布");
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_verify(args: VerifyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let template = load_template_file(&args.template)?;
    let pairs = pair_dirs(&args.samples, &args.expect_clean)?;
    let owned: Vec<_> = pairs
        .into_iter()
        .map(|(name, marked, clean)| (name, marked.rgba, clean.rgba))
        .collect();
    let report = verify_pairs(&template, &owned);
    print_verify(&report);
    if args.promote && report.accepted {
        promote_quality(&args.template)?;
        println!("已把 quality 写成 pixel");
    }
    if !report.accepted {
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_process(args: ProcessArgs) -> Result<(), Box<dyn std::error::Error>> {
    let templates = if args.no_watermark && args.template.is_none() && args.template_dir.is_none() {
        Vec::new()
    } else {
        load_requested(args.template.as_deref(), args.template_dir.as_deref())?
    };
    fs::create_dir_all(&args.out)?;
    let inputs = collect_inputs(&args.input)?;
    let bbox = args.bbox.as_deref().map(parse_bbox).transpose()?;
    let camera_time = args.camera_time.as_deref().map(parse_camera_time).transpose()?;
    let mut rows = Vec::new();
    let batch = inputs.len() > 1;
    for path in inputs {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("image").to_string();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        match decode_path(&path) {
            Ok(input) => {
                let opt = ProcessOptions {
                    accept_low_confidence: args.accept,
                    manual_bbox: bbox,
                    no_watermark: args.no_watermark,
                    lossy_jpeg: args.jpeg,
                    keep_exif: args.keep_exif,
                    ambiguous: false,
                    camera_time,
                };
                match process_best(&input, &templates, &opt) {
                    Ok(output) => {
                        let ext = if args.jpeg { "jpg" } else { "png" };
                        let image_path = args.out.join(format!("{stem}.restored.{ext}"));
                        let report_path = args.out.join(format!("{stem}.restored.report.json"));
                        if let Some(bytes) = &output.encoded {
                            fs::write(&image_path, bytes)?;
                        }
                        fs::write(&report_path, serde_json::to_string_pretty(&output.report)?)?;
                        println!("{name}\t{}", verdict_name(output.report.verdict));
                        rows.push(csv_row(&name, output.encoded.as_ref().map(|_| image_path.display().to_string()), &output));
                    }
                    Err(err) => {
                        eprintln!("{name}: {err}");
                        rows.push(format!("{name},,,failed,,,,,error"));
                    }
                }
            }
            Err(err) => {
                eprintln!("{name}: {err}");
                rows.push(format!("{name},,,failed,,,,,error"));
            }
        }
    }
    if batch {
        let mut csv = String::from(
            "input,output,template_id,template_version,verdict,ncc,alpha_mae,unstable_pixels,clamped_pixels,outside_diff,flags\n",
        );
        for row in rows {
            csv.push_str(&row);
            csv.push('\n');
        }
        fs::write(args.out.join("summary.csv"), csv)?;
    }
    Ok(())
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::PassWithUnstable => "pass_with_unstable",
        Verdict::NeedsReview => "needs_review",
        Verdict::NoWatermark => "no_watermark",
        Verdict::Unsupported => "unsupported",
        Verdict::Failed => "failed",
    }
}

fn csv_row(name: &str, output: Option<String>, result: &ProcessOutput) -> String {
    let r = &result.report;
    let flags = r.flags.join("|");
    format!(
        "{},{},{},{},{},{:.4},{:.4},{},{},{},{}",
        name,
        output.unwrap_or_default(),
        r.template_id.clone().unwrap_or_default(),
        r.template_version.map(|v| v.to_string()).unwrap_or_default(),
        verdict_name(r.verdict),
        r.ncc.unwrap_or(0.0),
        r.alpha_mae.unwrap_or(0.0),
        r.counts.unstable_pixels,
        r.counts.clamped_pixels,
        r.counts.outside_diff_pixels,
        flags
    )
}

fn load_images(paths: &[PathBuf]) -> Result<Vec<image::RgbaImage>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            for p in list_images(path)? {
                out.push(decode_path(&p)?.rgba);
            }
        } else {
            out.push(decode_path(path)?.rgba);
        }
    }
    Ok(out)
}

fn load_requested(file: Option<&Path>, dir: Option<&Path>) -> Result<Vec<LoadedTemplate>, Box<dyn std::error::Error>> {
    match (file, dir) {
        (Some(path), _) if path.is_dir() => Ok(load_template_dir(path)?),
        (Some(path), _) => Ok(vec![load_template_file(path)?]),
        (None, Some(dir)) => Ok(load_template_dir(dir)?),
        (None, None) => Err("需要 --template 或 --template-dir".into()),
    }
}

fn collect_inputs(path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if path.is_dir() {
        list_images(path)
    } else {
        Ok(vec![path.to_path_buf()])
    }
}

fn list_images(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
                Some("png") | Some("jpg") | Some("jpeg")
            )
        })
        .collect();
    files.sort();
    Ok(files)
}

fn pair_dirs(samples: &Path, clean: &Path) -> Result<Vec<(String, InputImage, InputImage)>, Box<dyn std::error::Error>> {
    let mut pairs = Vec::new();
    for sample in list_images(samples)? {
        let name = sample.file_name().unwrap();
        let expect = clean.join(name);
        if !expect.is_file() {
            return Err(format!("缺少干净原图 {}", expect.display()).into());
        }
        let marked = decode_path(&sample)?;
        let original = decode_path(&expect)?;
        pairs.push((name.to_string_lossy().into_owned(), marked, original));
    }
    if pairs.is_empty() {
        return Err("样本目录是空的".into());
    }
    Ok(pairs)
}

fn parse_camera_time(text: &str) -> Result<watermark_core::CameraTimeValue, Box<dyn std::error::Error>> {
    let (date, time) = text
        .split_once('T')
        .ok_or("相机时间用 2026-12-01T09:30，需要秒时写成 2026-12-01T09:30:45")?;
    let mut date_parts = date.split('-');
    let year: u16 = date_parts.next().ok_or("缺少年")?.parse()?;
    let month: u8 = date_parts.next().ok_or("缺少月")?.parse()?;
    let day: u8 = date_parts.next().ok_or("缺少日")?.parse()?;
    if date_parts.next().is_some() {
        return Err("日期用 YYYY-MM-DD".into());
    }
    let mut time_parts = time.split(':');
    let hour: u8 = time_parts.next().ok_or("缺少时")?.parse()?;
    let minute: u8 = time_parts.next().ok_or("缺少分")?.parse()?;
    let second = match time_parts.next() {
        Some(part) if !part.is_empty() => Some(part.parse::<u8>()?),
        _ => None,
    };
    if time_parts.next().is_some() {
        return Err("时间用 HH:MM 或 HH:MM:SS".into());
    }
    let value = watermark_core::CameraTimeValue { year, month, day, hour, minute, second };
    if !watermark_core::is_valid_camera_time(value) {
        return Err("相机时间不是合法的年月日时分秒".into());
    }
    Ok(value)
}

fn parse_bbox(text: &str) -> Result<[u32; 4], Box<dyn std::error::Error>> {
    let parts: Vec<_> = text.split(',').collect();
    if parts.len() != 4 {
        return Err("bbox 格式是 x,y,w,h".into());
    }
    let n = parts
        .iter()
        .map(|p| p.trim().parse::<u32>())
        .collect::<Result<Vec<_>, _>>()?;
    Ok([n[0], n[1], n[2], n[3]])
}

fn promote_quality(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut value: serde_json::Value = serde_json::from_str(&text)?;
    value["quality"] = serde_json::json!("pixel");
    fs::write(path, serde_json::to_string_pretty(&value)?)?;
    Ok(())
}

fn print_verify(report: &VerifyReport) {
    println!(
        "通过 {}/{}，比例 {:.1}%",
        report.passed,
        report.total,
        report.pass_rate * 100.0
    );
    for item in &report.items {
        println!("  {} {} {}", if item.passed { "通过" } else { "未过" }, item.name, item.detail);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiProbe {
    bbox: [i32; 4],
    alpha_base64: String,
    unstable_base64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponse {
    report: watermark_core::Report,
    image_base64: Option<String>,
    probe: Option<ApiProbe>,
    error: Option<String>,
}

fn serve(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let server = tiny_http::Server::http(bind).map_err(|e| format!("无法监听 {bind}: {e}"))?;
    println!("预览服务 http://{bind}");
    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let mut body = Vec::new();
        let _ = request.as_reader().read_to_end(&mut body);
        let (status, payload) = if method == tiny_http::Method::Options {
            (204, String::new())
        } else if url.starts_with("/api/health") {
            (200, "{\"ok\":true}".into())
        } else if url.starts_with("/api/sample") {
            (200, sample_payload())
        } else if url.starts_with("/api/templates") {
            (200, templates_payload())
        } else if url.starts_with("/api/process") && method == tiny_http::Method::Post {
            (200, handle_process(&body))
        } else if url.starts_with("/api/camera-time") && method == tiny_http::Method::Post {
            (200, handle_camera_time(&body))
        } else {
            (404, "{\"error\":\"未知路径\"}".into())
        };
        let mut response = tiny_http::Response::from_string(payload).with_status_code(status);
        response = add_cors(response);
        let _ = request.respond(response);
    }
    Ok(())
}

fn add_cors(response: tiny_http::Response<std::io::Cursor<Vec<u8>>>) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let header = |k: &str, v: &str| {
        tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("header")
    };
    response
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
}

fn decode_b64(text: &str) -> Result<Vec<u8>, String> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text.trim())
        .map_err(|e| format!("不是合法的 base64：{e}"))
}

fn demo_template() -> Result<LoadedTemplate, String> {
    let demo = watermark_core::fixture::demo();
    let png = watermark_core::encode_alpha16(&demo.template.alpha, demo.template.alpha_w, demo.template.alpha_h)
        .map_err(|e| e.to_string())?;
    let mut meta = demo.template.meta;
    meta.alpha_sha256 = watermark_core::sha256_hex(&png);
    let json = serde_json::to_string(&meta).map_err(|e| e.to_string())?;
    load_template_embedded_parts(&json, &png, None).map_err(|e| e.to_string())
}

fn load_part(json: &str, alpha_b64: &str, bias_b64: Option<&str>) -> Result<LoadedTemplate, String> {
    let alpha = decode_b64(alpha_b64)?;
    let bias = match bias_b64 {
        Some(text) if !text.trim().is_empty() => Some(decode_b64(text)?),
        _ => None,
    };
    load_template_embedded_parts(json, &alpha, bias.as_deref()).map_err(|e| e.to_string())
}

fn load_request_templates(req: &ProcessBody) -> Result<Vec<LoadedTemplate>, String> {
    if let Some(list) = &req.templates {
        if !list.is_empty() {
            let mut out = Vec::new();
            for part in list {
                out.push(load_part(&part.template_json, &part.alpha_base64, part.bias_base64.as_deref())?);
            }
            return Ok(out);
        }
    }
    match (&req.template_json, &req.alpha_base64) {
        (Some(json), Some(alpha)) => Ok(vec![load_part(json, alpha, req.bias_base64.as_deref())?]),
        _ => Ok(vec![demo_template()?]),
    }
}

fn has_template(dir: &Path) -> bool {
    dir.join("template_1024x1024.json").is_file()
}

fn bundled_template_dir() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("resources/doubao"));
            dirs.push(dir.join("doubao"));
            if let Some(parent) = dir.parent() {
                dirs.push(parent.join("resources/doubao"));
            }
        }
    }
    dirs.push(PathBuf::from("resources/doubao"));
    dirs.push(PathBuf::from("../resources/doubao"));
    dirs.into_iter().find(|dir| has_template(dir))
}

fn templates_payload() -> String {
    let Some(dir) = bundled_template_dir() else {
        return serde_json::json!({ "templates": [], "note": "没有找到豆包模板目录" }).to_string();
    };
    match read_template_files(&dir) {
        Ok(files) => {
            let templates: Vec<_> = files
                .into_iter()
                .map(|part| {
                    serde_json::json!({
                        "templateJson": part.json,
                        "alphaBase64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, part.alpha),
                        "biasBase64": part.bias.as_ref().map(|bytes| {
                            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
                        }),
                    })
                })
                .collect();
            serde_json::json!({
                "templates": templates,
                "note": "已载入豆包各尺寸模板。质量是视觉级，留出验证没有把它标成像素级。"
            })
            .to_string()
        }
        Err(err) => serde_json::json!({ "templates": [], "error": err.to_string() }).to_string(),
    }
}

fn sample_payload() -> String {
    let demo = watermark_core::fixture::demo();
    let image = watermark_core::encode_png(&demo.watermarked).unwrap_or_default();
    let alpha = watermark_core::encode_alpha16(&demo.template.alpha, demo.template.alpha_w, demo.template.alpha_h).unwrap_or_default();
    let mut meta = demo.template.meta;
    meta.alpha_sha256 = watermark_core::sha256_hex(&alpha);
    let body = serde_json::json!({
        "imageBase64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, image),
        "alphaBase64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, alpha),
        "templateJson": serde_json::to_string(&meta).unwrap_or_default(),
        "note": "合成示例，不是豆包或千问的真实水印"
    });
    body.to_string()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TemplatePart {
    template_json: String,
    alpha_base64: String,
    bias_base64: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessBody {
    image_base64: String,
    template_json: Option<String>,
    alpha_base64: Option<String>,
    bias_base64: Option<String>,
    templates: Option<Vec<TemplatePart>>,
    accept: Option<bool>,
    jpeg: Option<bool>,
    no_watermark: Option<bool>,
    keep_exif: Option<bool>,
    bbox: Option<[u32; 4]>,
    camera_time: Option<watermark_core::CameraTimeValue>,
}

fn handle_camera_time(body: &[u8]) -> String {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        image_base64: String,
    }
    let parsed: Body = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(err) => return serde_json::json!({"error": format!("请求不是合法 JSON：{err}")}).to_string(),
    };
    let bytes = match decode_b64(&parsed.image_base64) {
        Ok(value) => value,
        Err(err) => return serde_json::json!({"error": err}).to_string(),
    };
    let input = match decode_bytes(&bytes) {
        Ok(value) => value,
        Err(err) => return serde_json::json!({"error": err.to_string()}).to_string(),
    };
    match watermark_core::detect_camera_time(&input.rgba) {
        Some(found) => serde_json::json!({
            "found": true,
            "text": found.text,
            "year": found.year,
            "month": found.month,
            "day": found.day,
            "hour": found.hour,
            "minute": found.minute,
            "second": found.second,
            "separator": found.separator.to_string(),
        })
        .to_string(),
        None => serde_json::json!({"found": false}).to_string(),
    }
}

fn handle_process(body: &[u8]) -> String {
    let parsed: ProcessBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(err) => return serde_json::json!({"error": format!("请求不是合法 JSON：{err}")}).to_string(),
    };
    match run_request(&parsed) {
        Ok(text) => text,
        Err(err) => serde_json::json!({"error": err}).to_string(),
    }
}

fn run_request(req: &ProcessBody) -> Result<String, String> {
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        req.image_base64.trim(),
    )
    .map_err(|e| format!("图片不是合法 base64：{e}"))?;
    let input = decode_bytes(&bytes).map_err(|e| e.to_string())?;
    let templates = load_request_templates(req)?;
    let opt = ProcessOptions {
        accept_low_confidence: req.accept.unwrap_or(false),
        manual_bbox: req.bbox,
        no_watermark: req.no_watermark.unwrap_or(false),
        lossy_jpeg: req.jpeg.unwrap_or(false),
        keep_exif: req.keep_exif.unwrap_or(false),
        ambiguous: false,
        camera_time: req.camera_time,
    };
    let output = process_best(&input, &templates, &opt).map_err(|e| e.to_string())?;
    let probe = output.probe.as_ref().map(|p| ApiProbe {
        bbox: p.bbox,
        alpha_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &p.alpha),
        unstable_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &p.unstable),
    });
    let image_base64 = output
        .encoded
        .as_ref()
        .map(|b| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b));
    let response = ApiResponse {
        report: output.report,
        image_base64,
        probe,
        error: None,
    };
    serde_json::to_string(&response).map_err(|e| e.to_string())
}
