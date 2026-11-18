use crate::blend::{forward_u8, BlendMode};
use crate::color::{srgb_to_linear, ColorSpace};
use crate::template::{encode_alpha16, sample_alpha, sha256_hex, PlacementSpec, Quality, Template};
use crate::Error;
use image::RgbaImage;
use serde::Serialize;
use std::fs;
use std::path::Path;

pub struct CalibrateRequest {
    pub id: String,
    pub display_name: String,
    pub blacks: Vec<RgbaImage>,
    pub grays: Vec<RgbaImage>,
}

#[derive(Debug, Clone)]
pub struct CalibrateSummary {
    pub model_ok: bool,
    pub color_space: String,
    pub gray_mae: f32,
    pub gray_max: u8,
    pub template_count: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateReport {
    space: String,
    mae: f32,
    max: u8,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SizeReport {
    width: u32,
    height: u32,
    model_ok: bool,
    color_space: String,
    gray_mae: f32,
    gray_max: u8,
    template_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CalibrationReport {
    schema_version: u32,
    id: String,
    model_ok: bool,
    sizes: Vec<SizeReport>,
    candidates: Vec<CandidateReport>,
}

pub fn calibrate_to_dir(req: &CalibrateRequest, out_dir: &Path) -> Result<CalibrateSummary, Error> {
    if req.blacks.is_empty() || req.grays.is_empty() {
        return Err(Error::Template("标定至少需要一张黑图和一张灰图".into()));
    }
    fs::create_dir_all(out_dir)?;
    let mut groups: Vec<(u32, u32)> = Vec::new();
    for img in &req.blacks {
        let d = img.dimensions();
        if !groups.contains(&d) {
            groups.push(d);
        }
    }
    let mut sizes = Vec::new();
    let mut all_candidates = Vec::new();
    let mut model_ok_all = true;
    let group_count = groups.len();
    for (w, h) in groups {
        let blacks: Vec<&RgbaImage> = req.blacks.iter().filter(|i| i.dimensions() == (w, h)).collect();
        let grays: Vec<&RgbaImage> = req.grays.iter().filter(|i| i.dimensions() == (w, h)).collect();
        if grays.is_empty() {
            continue;
        }
        let built = build_one(&req.id, &req.display_name, &blacks, &grays)?;
        all_candidates.extend(built.candidates.clone());
        let suffix = if group_count == 1 {
            String::new()
        } else {
            format!("_w{w}")
        };
        let id = format!("{}{suffix}", req.id);
        let alpha_name = format!("alpha{suffix}.png");
        let json_name = if suffix.is_empty() {
            "template.json".to_string()
        } else {
            format!("template{suffix}.json")
        };
        let png = encode_alpha16(&built.alpha, built.aw, built.ah)?;
        let hash = sha256_hex(&png);
        let template = Template {
            schema_version: 1,
            id: id.clone(),
            display_name: req.display_name.clone(),
            version: 1,
            quality: Quality::Visual,
            blend_mode: BlendMode::SourceOver,
            color_space: built.space,
            color: [1.0, 1.0, 1.0],
            alpha_map: alpha_name.clone(),
            alpha_depth: 16,
            alpha_sha256: hash,
            placement: built.placement,
            alpha_stable_max: 0.80,
            min_image_size: [256, 256],
            ncc_auto: 0.85,
            ncc_confirm: 0.60,
        };
        fs::write(out_dir.join(&alpha_name), &png)?;
        fs::write(
            out_dir.join(&json_name),
            serde_json::to_string_pretty(&template).map_err(|e| Error::Template(e.to_string()))?,
        )?;
        if !built.model_ok {
            model_ok_all = false;
        }
        sizes.push(SizeReport {
            width: w,
            height: h,
            model_ok: built.model_ok,
            color_space: space_name(built.space).into(),
            gray_mae: built.mae,
            gray_max: built.max_err,
            template_id: id,
        });
    }
    if sizes.is_empty() {
        return Err(Error::Template("没有与黑图同尺寸的灰图".into()));
    }
    let model_ok = model_ok_all && sizes.iter().all(|s| s.model_ok);
    let report = CalibrationReport {
        schema_version: 1,
        id: req.id.clone(),
        model_ok,
        sizes: sizes.clone(),
        candidates: all_candidates,
    };
    fs::write(
        out_dir.join("calibration-report.json"),
        serde_json::to_string_pretty(&report).map_err(|e| Error::Template(e.to_string()))?,
    )?;
    let first = &sizes[0];
    Ok(CalibrateSummary {
        model_ok,
        color_space: first.color_space.clone(),
        gray_mae: first.gray_mae,
        gray_max: first.gray_max,
        template_count: sizes.len(),
    })
}

struct Built {
    alpha: Vec<f32>,
    aw: u32,
    ah: u32,
    space: ColorSpace,
    mae: f32,
    max_err: u8,
    model_ok: bool,
    placement: PlacementSpec,
    candidates: Vec<CandidateReport>,
}

fn build_one(id: &str, _name: &str, blacks: &[&RgbaImage], grays: &[&RgbaImage]) -> Result<Built, Error> {
    let _ = id;
    let reference = blacks[0];
    let (iw, ih) = reference.dimensions();
    let aligned: Vec<Vec<f32>> = blacks
        .iter()
        .map(|img| align_luma(reference, img))
        .collect();
    let median = median_luma(&aligned, iw, ih);
    let (x, y, bw, bh) = support_bbox(&median, iw, ih, 8.0 / 255.0)
        .ok_or_else(|| Error::Template("黑图上看不到水印".into()))?;
    let mut alpha_srgb = crop(&median, iw, x, y, bw, bh);
    for v in &mut alpha_srgb {
        if *v < 8.0 / 255.0 {
            *v = 0.0;
        }
    }
    // Gray plates share the black plate's pixel grid. Aligning them to the
    // black luma picks a wrong shift, because the backgrounds differ.
    let gray_stack: Vec<Vec<f32>> = if grays.len() == 1 {
        vec![luma(grays[0])]
    } else {
        grays.iter().map(|g| align_luma(grays[0], g)).collect()
    };
    let gray_med = median_luma(&gray_stack, iw, ih);
    let gray = crop(&gray_med, iw, x, y, bw, bh);

    let alpha_linear: Vec<f32> = alpha_srgb.iter().copied().map(srgb_to_linear).collect();
    let srgb_score = predict_mae(&alpha_srgb, &gray, bw, ColorSpace::Srgb);
    let linear_score = predict_mae(&alpha_linear, &gray, bw, ColorSpace::Linear);
    let candidates = vec![
        CandidateReport {
            space: "srgb".into(),
            mae: srgb_score.0,
            max: srgb_score.1,
        },
        CandidateReport {
            space: "linear".into(),
            mae: linear_score.0,
            max: linear_score.1,
        },
    ];
    let (space, mut alpha, mut mae, mut max_err) = if srgb_score.0 <= linear_score.0 {
        (ColorSpace::Srgb, alpha_srgb.clone(), srgb_score.0, srgb_score.1)
    } else {
        (ColorSpace::Linear, alpha_linear, linear_score.0, linear_score.1)
    };
    let (sdx, sdy, s_mae, s_max) = subpixel(&alpha, &gray, bw, bh, space);
    if s_mae <= mae {
        mae = s_mae;
        max_err = s_max;
    }
    // Keep the integer alpha map; the fractional search is stored as subpixel.
    let _ = &mut alpha;
    let model_ok = mae <= 2.0 && max_err <= 6;
    let m = iw.min(ih) as f32;
    let placement = PlacementSpec {
        anchor: "bottom-right".into(),
        size_basis: "min-side".into(),
        width_ratio: bw as f32 / m,
        aspect: bw as f32 / bh as f32,
        margin_x: (iw as f32 - x as f32 - bw as f32) / m,
        margin_y: (ih as f32 - y as f32 - bh as f32) / m,
        subpixel: [sdx, sdy],
        min_width_px: 48,
        padding_px: 4,
    };
    Ok(Built {
        alpha,
        aw: bw,
        ah: bh,
        space,
        mae,
        max_err,
        model_ok,
        placement,
        candidates,
    })
}

fn subpixel(alpha: &[f32], gray: &[f32], w: u32, h: u32, space: ColorSpace) -> (f32, f32, f32, u8) {
    let mut best = (0.0f32, 0.0f32, f32::MAX, 255u8);
    let mut step = -5;
    while step <= 5 {
        let d = step as f32 / 10.0;
        let mut sy = -5;
        while sy <= 5 {
            let dy = sy as f32 / 10.0;
            let shifted = shift_alpha(alpha, w, h, d, dy);
            let (mae, max_err) = predict_mae(&shifted, gray, w, space);
            if mae < best.2 {
                best = (d, dy, mae, max_err);
            }
            sy += 1;
        }
        step += 1;
    }
    best
}

fn shift_alpha(alpha: &[f32], w: u32, h: u32, dx: f32, dy: f32) -> Vec<f32> {
    let mut out = vec![0.0; alpha.len()];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = sample_alpha(alpha, w, h, x as f32 - dx, y as f32 - dy);
        }
    }
    out
}

fn predict_mae(alpha: &[f32], gray: &[f32], _w: u32, space: ColorSpace) -> (f32, u8) {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    let mut max_err = 0u8;
    for (&a, &g) in alpha.iter().zip(gray.iter()) {
        if a <= 0.02 {
            continue;
        }
        let obs = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
        let pred = forward_u8(128, a, 1.0, BlendMode::SourceOver, space);
        let err = pred.abs_diff(obs);
        max_err = max_err.max(err);
        sum += err as f32;
        n += 1.0;
    }
    if n == 0.0 {
        (99.0, 255)
    } else {
        (sum / n, max_err)
    }
}

fn align_luma(reference: &RgbaImage, image: &RgbaImage) -> Vec<f32> {
    let (w, h) = reference.dimensions();
    if image.dimensions() != (w, h) {
        return luma(reference);
    }
    if std::ptr::eq(reference, image) {
        return luma(reference);
    }
    let base = luma(reference);
    let other = luma(image);
    let (bx, by, bw, bh) = match support_bbox(&base, w, h, 8.0 / 255.0) {
        Some(v) => v,
        None => return other,
    };
    let mut best = (i32::MIN, 0i32, 0i32);
    for dy in -8..=8 {
        for dx in -8..=8 {
            let mut acc = 0i32;
            for py in 0..bh {
                for px in 0..bw {
                    let x = bx as i32 + px as i32 + dx;
                    let y = by as i32 + py as i32 + dy;
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                        continue;
                    }
                    let r = base[((by + py) * w + bx + px) as usize];
                    let o = other[(y as u32 * w + x as u32) as usize];
                    acc -= ((r - o).abs() * 255.0) as i32;
                }
            }
            if acc > best.0 {
                best = (acc, dx, dy);
            }
        }
    }
    let mut shifted = vec![0.0f32; base.len()];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let sx = x - best.1;
            let sy = y - best.2;
            let v = if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                other[(sy as u32 * w + sx as u32) as usize]
            } else {
                0.0
            };
            shifted[(y as u32 * w + x as u32) as usize] = v;
        }
    }
    shifted
}

fn luma(image: &RgbaImage) -> Vec<f32> {
    image
        .pixels()
        .map(|p| (p.0[0] as f32 + p.0[1] as f32 + p.0[2] as f32) / (3.0 * 255.0))
        .collect()
}

fn median_luma(images: &[Vec<f32>], w: u32, h: u32) -> Vec<f32> {
    let n = (w * h) as usize;
    let mut out = vec![0.0f32; n];
    let mut col = Vec::with_capacity(images.len());
    for i in 0..n {
        col.clear();
        for img in images {
            col.push(img[i]);
        }
        col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out[i] = col[col.len() / 2];
    }
    out
}

fn support_bbox(luma: &[f32], w: u32, h: u32, thresh: f32) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if luma[(y * w + x) as usize] > thresh {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any || max_x < min_x || max_y < min_y {
        return None;
    }
    Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

fn crop(src: &[f32], stride: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((w * h) as usize);
    for py in 0..h {
        for px in 0..w {
            out.push(src[((y + py) * stride + x + px) as usize]);
        }
    }
    out
}

fn space_name(space: ColorSpace) -> &'static str {
    match space {
        ColorSpace::Srgb => "srgb",
        ColorSpace::Linear => "linear",
    }
}
