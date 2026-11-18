//! Synthetic template used by tests and the local preview.
//! It is not a Doubao or Qwen watermark.

use crate::blend::BlendMode;
use crate::color::ColorSpace;
use crate::image_io::solid;
use crate::placement::place;
use crate::restore::composite_alpha;
use crate::template::{encode_alpha16, sha256_hex, LoadedTemplate, PlacementSpec, Quality, Template};
use crate::Error;
use image::{Rgba, RgbaImage};
use std::fs;
use std::path::Path;

pub struct Demo {
    pub template: LoadedTemplate,
    pub clean: RgbaImage,
    pub watermarked: RgbaImage,
}

pub fn demo() -> Demo {
    demo_sized(640, 480, 96, 48)
}

pub fn demo_sized(image_w: u32, image_h: u32, aw: u32, ah: u32) -> Demo {
    let alpha = bar_alpha(aw, ah, 0.50);
    let template = template_from_alpha(alpha, aw, ah, image_w, image_h, Quality::Pixel);
    let clean = scene(image_w, image_h);
    let mut watermarked = clean.clone();
    let placed = place(&template.meta, image_w, image_h).expect("demo placement");
    assert_eq!(placed.width, aw, "placement width drifted");
    assert_eq!(placed.height, ah, "placement height drifted");
    composite_alpha(
        &mut watermarked,
        &template.alpha,
        placed.origin_x.floor() as i32,
        placed.origin_y.floor() as i32,
        placed.width,
        template.meta.color,
        template.meta.blend_mode,
        template.meta.color_space,
    );
    Demo {
        template,
        clean,
        watermarked,
    }
}

pub fn bar_alpha(w: u32, h: u32, peak: f32) -> Vec<f32> {
    let mut a = vec![0.0f32; (w * h) as usize];
    let y0 = h / 5;
    let y1 = h - h / 5;
    let bars = [
        (w / 8, w / 8 + w / 10, peak * 0.45),
        (w / 3, w / 3 + w / 6, peak * 0.85),
        (w * 2 / 3, w * 2 / 3 + w / 12, peak),
    ];
    let span = (y1 - y0).max(1) as f32;
    for y in y0..y1 {
        let fade = 0.75 + 0.25 * ((y - y0) as f32 / span);
        for (x0, x1, level) in bars {
            for x in x0..x1.min(w) {
                a[(y * w + x) as usize] = (level * fade).min(0.75);
            }
        }
    }
    a
}

pub fn template_from_alpha(
    alpha: Vec<f32>,
    aw: u32,
    ah: u32,
    image_w: u32,
    image_h: u32,
    quality: Quality,
) -> LoadedTemplate {
    let m = image_w.min(image_h) as f32;
    let mut width_ratio = aw as f32 / m;
    for _ in 0..8 {
        if (width_ratio * m).round() as u32 == aw {
            break;
        }
        width_ratio += 1.0 / m;
    }
    let aspect = aw as f32 / ah as f32;
    let meta = Template {
        schema_version: 1,
        id: "synthetic_v1".into(),
        display_name: "合成示例水印".into(),
        version: 1,
        quality,
        blend_mode: BlendMode::SourceOver,
        color_space: ColorSpace::Srgb,
        color: [1.0, 1.0, 1.0],
        alpha_map: "alpha_maps/synthetic_v1.png".into(),
        alpha_depth: 16,
        alpha_sha256: String::new(),
        placement: PlacementSpec {
            anchor: "bottom-right".into(),
            size_basis: "min-side".into(),
            width_ratio,
            aspect,
            margin_x: 0.0,
            margin_y: 0.0,
            subpixel: [0.0, 0.0],
            min_width_px: 48,
            padding_px: 4,
        },
        alpha_stable_max: 0.80,
        min_image_size: [256, 256],
        ncc_auto: 0.85,
        ncc_confirm: 0.60,
    };
    LoadedTemplate {
        meta,
        alpha,
        alpha_w: aw,
        alpha_h: ah,
        source: None,
    }
}

pub fn scene(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (48.0 + 90.0 * x as f32 / w as f32) as u8;
            let g = (42.0 + 70.0 * y as f32 / h as f32) as u8;
            let b = (70.0 + 40.0 * (x as f32 / w as f32)) as u8;
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let rad = (w.min(h) as i32) / 7;
    for y in (cy - rad).max(0)..(cy + rad).min(h as i32) {
        for x in (cx - rad).max(0)..(cx + rad).min(w as i32) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= rad * rad {
                img.put_pixel(x as u32, y as u32, Rgba([210, 96, 72, 255]));
            }
        }
    }
    img
}

pub fn write_resources(root: &Path) -> Result<(), Error> {
    let demo = demo();
    let png = encode_alpha16(&demo.template.alpha, demo.template.alpha_w, demo.template.alpha_h)?;
    let hash = sha256_hex(&png);
    let mut meta = demo.template.meta.clone();
    meta.alpha_sha256 = hash;
    let templates = root.join("resources/templates");
    let maps = root.join("resources/alpha_maps");
    fs::create_dir_all(&templates)?;
    fs::create_dir_all(&maps)?;
    fs::write(maps.join("synthetic_v1.png"), png)?;
    fs::write(
        templates.join("synthetic_v1.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| Error::Template(e.to_string()))?,
    )?;
    fs::create_dir_all(root.join("resources/samples/marked"))?;
    fs::create_dir_all(root.join("resources/samples/clean"))?;
    let marked = encode_png_rgba(&demo.watermarked)?;
    let clean = encode_png_rgba(&demo.clean)?;
    fs::write(root.join("resources/samples/synthetic.png"), &marked)?;
    fs::write(root.join("resources/samples/marked/synthetic.png"), &marked)?;
    fs::write(root.join("resources/samples/clean/synthetic.png"), &clean)?;
    Ok(())
}

fn encode_png_rgba(img: &RgbaImage) -> Result<Vec<u8>, Error> {
    crate::image_io::encode_png(img)
}

/// Flat plate with a bottom-right block, for calibration tests.
pub fn plate(w: u32, h: u32, rgb: [u8; 3], alpha: f32, bw: u32, bh: u32) -> RgbaImage {
    let mut img = solid(w, h, rgb);
    let aw = bw;
    let ah = bh;
    let alpha_map = vec![alpha; (aw * ah) as usize];
    composite_alpha(
        &mut img,
        &alpha_map,
        (w - bw) as i32,
        (h - bh) as i32,
        bw,
        [1.0, 1.0, 1.0],
        BlendMode::SourceOver,
        ColorSpace::Srgb,
    );
    img
}

pub fn linear_plate(w: u32, h: u32, level: u8, alpha: f32, bw: u32, bh: u32) -> RgbaImage {
    let mut img = solid(w, h, [level, level, level]);
    let alpha_map = vec![alpha; (bw * bh) as usize];
    composite_alpha(
        &mut img,
        &alpha_map,
        (w - bw) as i32,
        (h - bh) as i32,
        bw,
        [1.0, 1.0, 1.0],
        BlendMode::SourceOver,
        ColorSpace::Linear,
    );
    img
}
