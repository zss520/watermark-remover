use crate::image_io::InputImage;
use crate::metrics::{max_abs, psnr_rgb, ssim_min_rgb};
use crate::report::HoldoutMetrics;
use crate::restore::{process_one, Coverage, ProcessOptions, ProcessOutput};
use crate::template::LoadedTemplate;
use image::RgbaImage;

#[derive(Debug, Clone)]
pub struct VerifyItem {
    pub name: String,
    pub passed: bool,
    pub metrics: Option<HoldoutMetrics>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub total: usize,
    pub passed: usize,
    pub pass_rate: f64,
    pub accepted: bool,
    pub items: Vec<VerifyItem>,
}

pub fn verify_pairs(
    template: &LoadedTemplate,
    pairs: &[(String, RgbaImage, RgbaImage)],
) -> VerifyReport {
    let mut items = Vec::with_capacity(pairs.len());
    for (name, watermarked, clean) in pairs {
        items.push(verify_one(template, name, watermarked, clean));
    }
    let passed = items.iter().filter(|i| i.passed).count();
    let total = items.len();
    let pass_rate = if total == 0 { 0.0 } else { passed as f64 / total as f64 };
    VerifyReport {
        total,
        passed,
        pass_rate,
        accepted: total > 0 && pass_rate >= 0.95,
        items,
    }
}

fn verify_one(template: &LoadedTemplate, name: &str, watermarked: &RgbaImage, clean: &RgbaImage) -> VerifyItem {
    if watermarked.dimensions() != clean.dimensions() {
        return VerifyItem {
            name: name.into(),
            passed: false,
            metrics: None,
            detail: "尺寸不一致".into(),
        };
    }
    let input = InputImage {
        rgba: watermarked.clone(),
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let mut opt = ProcessOptions::default();
    opt.accept_low_confidence = true;
    let output = match process_one(&input, template, &opt) {
        Ok(v) => v,
        Err(e) => {
            return VerifyItem {
                name: name.into(),
                passed: false,
                metrics: None,
                detail: e.to_string(),
            }
        }
    };
    judge_output(name, clean, &output, template.meta.alpha_stable_max)
}

pub fn judge_output(name: &str, clean: &RgbaImage, output: &ProcessOutput, stable_max: f32) -> VerifyItem {
    let (Some(restored), Some(coverage)) = (&output.image, &output.coverage) else {
        return VerifyItem {
            name: name.into(),
            passed: false,
            metrics: None,
            detail: format!("没有输出：{:?}", output.report.verdict),
        };
    };
    let outside = output.report.counts.outside_diff_pixels;
    let (metrics, passed) = judge_holdout(
        clean,
        restored,
        coverage,
        stable_max,
        ratio(output.report.counts.unstable_pixels, output.report.counts.core_pixels),
        output.report.counts.clamped_pixels,
        outside,
    );
    VerifyItem {
        name: name.into(),
        passed,
        detail: format!(
            "psnr={:.2} ssim={:.4} max={} stable={} outside={}",
            metrics.psnr, metrics.ssim_min, metrics.max_abs, metrics.stable_max_abs, outside
        ),
        metrics: Some(metrics),
    }
}

fn ratio(unstable: u32, core: u32) -> f32 {
    if core == 0 {
        1.0
    } else {
        unstable as f32 / core as f32
    }
}

pub fn judge_holdout(
    clean: &RgbaImage,
    restored: &RgbaImage,
    coverage: &Coverage,
    stable_max: f32,
    unstable_ratio: f32,
    clamped: u32,
    outside: u32,
) -> (HoldoutMetrics, bool) {
    let mut stable_clean = Vec::new();
    let mut stable_restored = Vec::new();
    let mut region_clean = Vec::new();
    let mut region_restored = Vec::new();
    let mut stable_max_abs = 0u8;
    let mut region_max = 0u8;
    for y in 0..coverage.height {
        for x in 0..coverage.width {
            let i = (y * coverage.width + x) as usize;
            let ix = coverage.x + x as i32;
            let iy = coverage.y + y as i32;
            if ix < 0 || iy < 0 || ix >= clean.width() as i32 || iy >= clean.height() as i32 {
                continue;
            }
            let c = clean.get_pixel(ix as u32, iy as u32).0;
            let r = restored.get_pixel(ix as u32, iy as u32).0;
            if coverage.evaluate.get(i) != Some(&true) {
                continue;
            }
            region_clean.extend_from_slice(&c[..3]);
            region_restored.extend_from_slice(&r[..3]);
            region_max = region_max.max(max_abs(&c[..3], &r[..3]));
            let a = coverage.alpha.get(i).copied().unwrap_or(0.0);
            if a > 0.02 && a <= stable_max {
                stable_clean.extend_from_slice(&c[..3]);
                stable_restored.extend_from_slice(&r[..3]);
                stable_max_abs = stable_max_abs.max(max_abs(&c[..3], &r[..3]));
            }
        }
    }
    let psnr = if stable_clean.is_empty() {
        0.0
    } else {
        psnr_rgb(&stable_clean, &stable_restored)
    };
    let (crop_c, crop_r) = crop_rgb(clean, restored, coverage);
    let ssim = if coverage.width >= 11 && coverage.height >= 11 {
        ssim_min_rgb(&crop_c, &crop_r, coverage.width, coverage.height)
    } else if crop_c == crop_r {
        1.0
    } else {
        0.0
    };
    let short = coverage.width.min(coverage.height);
    let ssim_ok = short < 64 || ssim > 0.999;
    let passed = outside == 0
        && clamped == 0
        && unstable_ratio <= 0.02
        && !stable_clean.is_empty()
        && stable_max_abs <= 2
        && psnr > 50.0
        && ssim_ok;
    (
        HoldoutMetrics {
            psnr,
            ssim_min: ssim,
            max_abs: region_max,
            stable_max_abs,
        },
        passed,
    )
}

fn crop_rgb(clean: &RgbaImage, restored: &RgbaImage, coverage: &Coverage) -> (Vec<u8>, Vec<u8>) {
    let mut a = Vec::with_capacity((coverage.width * coverage.height * 3) as usize);
    let mut b = Vec::with_capacity(a.capacity());
    for y in 0..coverage.height {
        for x in 0..coverage.width {
            let ix = coverage.x + x as i32;
            let iy = coverage.y + y as i32;
            let (ca, cb) = if ix >= 0 && iy >= 0 && (ix as u32) < clean.width() && (iy as u32) < clean.height() {
                (
                    clean.get_pixel(ix as u32, iy as u32).0,
                    restored.get_pixel(ix as u32, iy as u32).0,
                )
            } else {
                ([0, 0, 0, 255], [0, 0, 0, 255])
            };
            a.extend_from_slice(&ca[..3]);
            b.extend_from_slice(&cb[..3]);
        }
    }
    (a, b)
}
