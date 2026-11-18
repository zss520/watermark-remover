use base64::Engine;
use serde::{Deserialize, Serialize};
use watermark_core::{
    decode_bytes, encode_alpha16, encode_png, load_template_embedded, process_best, sha256_hex, LoadedTemplate,
    ProcessOptions,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TemplatePart {
    template_json: String,
    alpha_base64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessBody {
    image_base64: String,
    template_json: Option<String>,
    alpha_base64: Option<String>,
    templates: Option<Vec<TemplatePart>>,
    accept: Option<bool>,
    jpeg: Option<bool>,
    no_watermark: Option<bool>,
    keep_exif: Option<bool>,
    bbox: Option<[u32; 4]>,
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
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|err| format!("不是合法的 base64：{err}"))
}

fn demo_loaded() -> Result<LoadedTemplate, String> {
    let (json, png) = demo_template()?;
    load_template_embedded(&json, &png).map_err(|err| err.to_string())
}

fn load_body_templates(body: &ProcessBody) -> Result<Vec<LoadedTemplate>, String> {
    if let Some(list) = &body.templates {
        if !list.is_empty() {
            let mut out = Vec::new();
            for part in list {
                let alpha = b64_decode(&part.alpha_base64)?;
                out.push(load_template_embedded(&part.template_json, &alpha).map_err(|err| err.to_string())?);
            }
            return Ok(out);
        }
    }
    match (&body.template_json, &body.alpha_base64) {
        (Some(json), Some(alpha)) => {
            let alpha = b64_decode(alpha)?;
            Ok(vec![load_template_embedded(json, &alpha).map_err(|err| err.to_string())?])
        }
        _ => Ok(vec![demo_loaded()?]),
    }
}

fn demo_template() -> Result<(String, Vec<u8>), String> {
    let demo = watermark_core::fixture::demo();
    let png = encode_alpha16(&demo.template.alpha, demo.template.alpha_w, demo.template.alpha_h)
        .map_err(|err| err.to_string())?;
    let mut meta = demo.template.meta;
    meta.alpha_sha256 = sha256_hex(&png);
    let json = serde_json::to_string(&meta).map_err(|err| err.to_string())?;
    Ok((json, png))
}

#[tauri::command]
fn sample_image() -> Result<serde_json::Value, String> {
    let demo = watermark_core::fixture::demo();
    let image = encode_png(&demo.watermarked).map_err(|err| err.to_string())?;
    let (json, alpha) = demo_template()?;
    Ok(serde_json::json!({
        "imageBase64": b64_encode(&image),
        "alphaBase64": b64_encode(&alpha),
        "templateJson": json,
        "note": "合成示例，不是豆包或千问的真实水印"
    }))
}

#[tauri::command]
fn process_image(body: ProcessBody) -> Result<ApiResponse, String> {
    let bytes = b64_decode(&body.image_base64)?;
    let input = decode_bytes(&bytes).map_err(|err| err.to_string())?;
    let templates = load_body_templates(&body)?;
    let output = process_best(
        &input,
        &templates,
        &ProcessOptions {
            accept_low_confidence: body.accept.unwrap_or(false),
            manual_bbox: body.bbox,
            no_watermark: body.no_watermark.unwrap_or(false),
            lossy_jpeg: body.jpeg.unwrap_or(false),
            keep_exif: body.keep_exif.unwrap_or(false),
            ambiguous: false,
        },
    )
    .map_err(|err| err.to_string())?;
    let probe = output.probe.as_ref().map(|probe| ApiProbe {
        bbox: probe.bbox,
        alpha_base64: b64_encode(&probe.alpha),
        unstable_base64: b64_encode(&probe.unstable),
    });
    Ok(ApiResponse {
        report: output.report,
        image_base64: output.encoded.as_ref().map(|bytes| b64_encode(bytes)),
        probe,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![sample_image, process_image])
        .run(tauri::generate_context!())
        .expect("水印去除窗口启动失败");
}
