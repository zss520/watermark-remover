use crate::blend::BlendMode;
use crate::color::ColorSpace;
use crate::Error;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Pixel,
    Visual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementSpec {
    pub anchor: String,
    pub size_basis: String,
    pub width_ratio: f32,
    pub aspect: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub subpixel: [f32; 2],
    pub min_width_px: u32,
    pub padding_px: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Template {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub version: u32,
    pub quality: Quality,
    pub blend_mode: BlendMode,
    pub color_space: ColorSpace,
    pub color: [f32; 3],
    pub alpha_map: String,
    pub alpha_depth: u32,
    pub alpha_sha256: String,
    pub placement: PlacementSpec,
    pub alpha_stable_max: f32,
    pub min_image_size: [u32; 2],
    pub ncc_auto: f32,
    pub ncc_confirm: f32,
}

#[derive(Debug, Clone)]
pub struct LoadedTemplate {
    pub meta: Template,
    pub alpha: Vec<f32>,
    pub alpha_w: u32,
    pub alpha_h: u32,
    pub source: Option<PathBuf>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let dig = Sha256::digest(bytes);
    let mut s = String::with_capacity(dig.len() * 2);
    for b in dig {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn load_template_file(path: &Path) -> Result<LoadedTemplate, Error> {
    let text = fs::read_to_string(path)?;
    let meta: Template = serde_json::from_str(&text).map_err(|e| Error::Schema(e.to_string()))?;
    validate_meta(&meta)?;
    let alpha_path = resolve_alpha(path, &meta.alpha_map)?;
    let bytes = fs::read(&alpha_path)?;
    load_with_alpha(meta, &bytes, Some(path.to_path_buf()))
}

pub fn load_template_embedded(json: &str, alpha_png: &[u8]) -> Result<LoadedTemplate, Error> {
    let meta: Template = serde_json::from_str(json).map_err(|e| Error::Schema(e.to_string()))?;
    validate_meta(&meta)?;
    load_with_alpha(meta, alpha_png, None)
}

pub fn load_template_dir(dir: &Path) -> Result<Vec<LoadedTemplate>, Error> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(Error::Template(format!("{} 里没有模板 json", dir.display())));
    }
    files.into_iter().map(|p| load_template_file(&p)).collect()
}

fn load_with_alpha(
    meta: Template,
    alpha_png: &[u8],
    source: Option<PathBuf>,
) -> Result<LoadedTemplate, Error> {
    let got = sha256_hex(alpha_png);
    if got != meta.alpha_sha256.to_ascii_lowercase() {
        return Err(Error::HashMismatch);
    }
    let (alpha, w, h) = decode_alpha(alpha_png, meta.alpha_depth)?;
    Ok(LoadedTemplate {
        meta,
        alpha,
        alpha_w: w,
        alpha_h: h,
        source,
    })
}

fn validate_meta(meta: &Template) -> Result<(), Error> {
    if meta.schema_version != 1 {
        return Err(Error::Template("schemaVersion 只接受 1".into()));
    }
    if meta.placement.anchor != "bottom-right" || meta.placement.size_basis != "min-side" {
        return Err(Error::Template(
            "v1 只接受 anchor=bottom-right 且 sizeBasis=min-side".into(),
        ));
    }
    if meta.placement.aspect <= 0.0 || meta.placement.width_ratio <= 0.0 {
        return Err(Error::Template("placement 的宽高比无效".into()));
    }
    if meta.alpha_depth != 16 && meta.alpha_depth != 8 {
        return Err(Error::Template("alphaDepth 只接受 8 或 16".into()));
    }
    if meta.color.iter().any(|c| !(0.0..=1.0).contains(c)) {
        return Err(Error::Template("color 必须在 0 到 1".into()));
    }
    if !(0.0..=1.0).contains(&meta.alpha_stable_max) {
        return Err(Error::Template("alphaStableMax 必须在 0 到 1".into()));
    }
    if meta.ncc_confirm > meta.ncc_auto {
        return Err(Error::Template("nccConfirm 不能高于 nccAuto".into()));
    }
    Ok(())
}

fn resolve_alpha(json_path: &Path, alpha_map: &str) -> Result<PathBuf, Error> {
    let rel = Path::new(alpha_map);
    if rel.is_absolute() {
        return Ok(rel.to_path_buf());
    }
    let parent = json_path.parent().unwrap_or_else(|| Path::new("."));
    let beside = parent.join(rel);
    if beside.is_file() {
        return Ok(beside);
    }
    // templates/foo.json + alpha_maps/foo.png resolves against resources/.
    if let Some(grand) = parent.parent() {
        let up = grand.join(rel);
        if up.is_file() {
            return Ok(up);
        }
    }
    Err(Error::Template(format!("找不到 Alpha 贴图 {alpha_map}")))
}

fn decode_alpha(bytes: &[u8], depth: u32) -> Result<(Vec<f32>, u32, u32), Error> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Template(e.to_string()))?;
    let w = img.width();
    let h = img.height();
    let alpha = match (&img, depth) {
        (image::DynamicImage::ImageLuma16(buf), 16) => buf.pixels().map(|p| p.0[0] as f32 / 65535.0).collect(),
        (image::DynamicImage::ImageLuma8(buf), 8) => buf.pixels().map(|p| p.0[0] as f32 / 255.0).collect(),
        (image::DynamicImage::ImageLuma16(_), 8) => {
            return Err(Error::Template("模板声明 8-bit，文件却是 16-bit".into()))
        }
        (image::DynamicImage::ImageLuma8(_), 16) => {
            return Err(Error::Template("模板声明 16-bit，文件却是 8-bit".into()))
        }
        _ => return Err(Error::Template("Alpha 贴图必须是灰度 PNG".into())),
    };
    Ok((alpha, w, h))
}

pub fn encode_alpha16(alpha: &[f32], w: u32, h: u32) -> Result<Vec<u8>, Error> {
    if alpha.len() != (w * h) as usize {
        return Err(Error::Template("Alpha 尺寸与缓冲区不一致".into()));
    }
    let mut buf = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(w, h);
    for (i, px) in alpha.iter().enumerate() {
        let v = (px.clamp(0.0, 1.0) * 65535.0).round() as u16;
        let x = (i as u32) % w;
        let y = (i as u32) / w;
        buf.put_pixel(x, y, image::Luma([v]));
    }
    let mut cursor = std::io::Cursor::new(Vec::new());
    buf.write_with_encoder(image::codecs::png::PngEncoder::new(&mut cursor))
        .map_err(|e| Error::Template(e.to_string()))?;
    Ok(cursor.into_inner())
}

/// Bilinear sample. Integer coordinates land on pixel centers of the sprite.
pub fn sample_alpha(alpha: &[f32], aw: u32, ah: u32, x: f32, y: f32) -> f32 {
    if aw == 0 || ah == 0 {
        return 0.0;
    }
    if x < -0.5 || y < -0.5 || x > aw as f32 - 0.5 || y > ah as f32 - 0.5 {
        return 0.0;
    }
    let x = x.clamp(0.0, aw as f32 - 1.0);
    let y = y.clamp(0.0, ah as f32 - 1.0);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(aw - 1);
    let y1 = (y0 + 1).min(ah - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let at = |px: u32, py: u32| alpha[(py * aw + px) as usize];
    let a = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
    let b = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
    a * (1.0 - ty) + b * ty
}

/// Rasterize the sprite into the placed pixel grid.
/// `frac` is the subpixel part of the origin; integer shifts do not change it.
pub fn rasterize_alpha(tpl: &LoadedTemplate, placed_w: u32, placed_h: u32, frac_x: f32, frac_y: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; (placed_w * placed_h) as usize];
    if placed_w == tpl.alpha_w && placed_h == tpl.alpha_h && frac_x.abs() < 1e-4 && frac_y.abs() < 1e-4 {
        out.copy_from_slice(&tpl.alpha);
        return out;
    }
    for py in 0..placed_h {
        for px in 0..placed_w {
            let sx = (px as f32 + 0.5 - frac_x) * (tpl.alpha_w as f32 / placed_w as f32) - 0.5;
            let sy = (py as f32 + 0.5 - frac_y) * (tpl.alpha_h as f32 / placed_h as f32) - 0.5;
            out[(py * placed_w + px) as usize] = sample_alpha(&tpl.alpha, tpl.alpha_w, tpl.alpha_h, sx, sy);
        }
    }
    out
}
