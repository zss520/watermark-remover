use crate::blend::{inverse_u8, BlendMode};
use crate::camera_time::{self, CameraTimeError, CameraTimeValue};
use crate::color::ColorSpace;
use crate::image_io::{encode_jpeg, encode_png, patch_exif_orientation_to_top_left, png_insert_chunk, InputImage};
use crate::locate::{locate, manual_match, Match};
use crate::placement::Placement;
use crate::report::{confirmation_required, decide, Counts, Decision, InputInfo, OutputInfo, Report, Verdict};
use crate::template::LoadedTemplate;
use crate::Error;
use image::RgbaImage;

#[derive(Debug, Clone)]
pub struct ProcessOptions {
    pub accept_low_confidence: bool,
    pub manual_bbox: Option<[u32; 4]>,
    pub no_watermark: bool,
    pub lossy_jpeg: bool,
    pub keep_exif: bool,
    pub ambiguous: bool,
    /// When set, rewrite a detected camera date after the watermark step.
    /// `None` leaves every pixel the watermark step produced.
    pub camera_time: Option<CameraTimeValue>,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            accept_low_confidence: false,
            manual_bbox: None,
            no_watermark: false,
            lossy_jpeg: false,
            keep_exif: false,
            ambiguous: false,
            camera_time: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub bbox: [i32; 4],
    pub alpha: Vec<u8>,
    pub unstable: Vec<u8>,
}

/// Crop covering the dilated evaluation mask. `alpha` is 0 on padding pixels.
#[derive(Debug, Clone)]
pub struct Coverage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<f32>,
    pub evaluate: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub image: Option<RgbaImage>,
    pub encoded: Option<Vec<u8>>,
    pub report: Report,
    pub probe: Option<Probe>,
    pub coverage: Option<Coverage>,
}

pub fn process_one(
    input: &InputImage,
    template: &LoadedTemplate,
    opt: &ProcessOptions,
) -> Result<ProcessOutput, Error> {
    let output = if opt.no_watermark {
        copy_through(input, opt)?
    } else {
        let found = if let Some(bbox) = opt.manual_bbox {
            manual_match(template, &input.rgba, bbox)
        } else {
            locate(&input.rgba, template)
        };
        restore_match(input, template, opt, found)?
    };
    apply_camera_time(input, opt, output)
}

pub fn process_best(
    input: &InputImage,
    templates: &[LoadedTemplate],
    opt: &ProcessOptions,
) -> Result<ProcessOutput, Error> {
    if opt.no_watermark {
        let output = copy_through(input, opt)?;
        return apply_camera_time(input, opt, output);
    }
    if templates.is_empty() {
        return Err(Error::Template("没有可用模板".into()));
    }
    let dims = [input.rgba.width(), input.rgba.height()];
    let matched: Vec<usize> = templates
        .iter()
        .enumerate()
        .filter(|(_, t)| t.meta.source_size == Some(dims))
        .map(|(i, _)| i)
        .collect();
    let pool: Vec<usize> = if matched.is_empty() {
        (0..templates.len()).collect()
    } else {
        matched
    };
    if pool.len() == 1 || opt.manual_bbox.is_some() {
        return process_one(input, &templates[pool[0]], opt);
    }
    let mut scored: Vec<(f32, usize)> = pool
        .iter()
        .map(|&i| (locate(&input.rgba, &templates[i]).ncc, i))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let best = scored[0];
    let second = scored.get(1).copied();
    let confirm = templates[best.1].meta.ncc_confirm;
    let ambiguous = match second {
        Some((ncc2, _)) if best.0 >= confirm && ncc2 >= confirm && (best.0 - ncc2) < 0.03 => true,
        _ => false,
    };
    let mut opt = opt.clone();
    opt.ambiguous = ambiguous;
    process_one(input, &templates[best.1], &opt)
}

fn restore_match(
    input: &InputImage,
    template: &LoadedTemplate,
    opt: &ProcessOptions,
    found: Match,
) -> Result<ProcessOutput, Error> {
    let manual = opt.manual_bbox.is_some();
    let base_decision = Decision {
        ncc: found.ncc,
        ncc_auto: template.meta.ncc_auto,
        ncc_confirm: template.meta.ncc_confirm,
        applicable: found.applicable,
        accept_low: opt.accept_low_confidence,
        manual,
        no_watermark: false,
        quality_pixel: matches!(template.meta.quality, crate::template::Quality::Pixel),
        core: 0,
        unstable: 0,
        clamped: 0,
        outside: 0,
        precision_reduced: input.precision_reduced,
        non_srgb: input.non_srgb,
        lossy: opt.lossy_jpeg,
        ambiguous: opt.ambiguous,
    };
    let (early, early_flags) = decide(&base_decision);
    if matches!(early, Verdict::Unsupported)
        || (confirmation_required(&early_flags) && !manual)
    {
        let report = skeleton(input, template, &found, early, early_flags, Counts::zero(), None);
        let probe = found.placement.as_ref().map(|p| probe_from(&found, p));
        return Ok(ProcessOutput {
            image: None,
            encoded: None,
            report,
            probe,
            coverage: None,
        });
    }

    let place = found.placement.clone().ok_or_else(|| Error::Template("没有放置结果".into()))?;
    let (mut image, counts, coverage, probe) = invert(input, template, &place, &found.alpha, &found.bias);
    let outside = outside_diff(&input.rgba, &image, &coverage);
    // The inverter only writes evaluation-mask pixels. A non-zero diff means
    // the invariant broke; drop the pixels and fail the report.
    if outside > 0 {
        image = input.rgba.clone();
    }
    let decision = Decision {
        core: counts.core_pixels,
        unstable: counts.unstable_pixels,
        clamped: counts.clamped_pixels,
        outside,
        ..base_decision
    };
    let (verdict, flags) = decide(&decision);
    let mut counts = counts;
    counts.outside_diff_pixels = outside;
    let keep_image = !matches!(verdict, Verdict::Failed | Verdict::Unsupported)
        && !confirmation_required(&flags);
    let encoded = if keep_image {
        Some(encode_result(&image, input, opt)?)
    } else {
        None
    };
    let output = encoded.as_ref().map(|_| OutputInfo {
        format: if opt.lossy_jpeg { "jpeg".into() } else { "png".into() },
        lossy: opt.lossy_jpeg,
    });
    let report = Report {
        output,
        verdict,
        flags,
        counts,
        ..skeleton(input, template, &found, Verdict::Failed, Vec::new(), Counts::zero(), None)
    };
    Ok(ProcessOutput {
        image: if keep_image { Some(image) } else { None },
        encoded,
        report,
        probe: Some(probe),
        coverage: Some(coverage),
    })
}

fn skeleton(
    input: &InputImage,
    template: &LoadedTemplate,
    found: &Match,
    verdict: Verdict,
    flags: Vec<String>,
    counts: Counts,
    output: Option<OutputInfo>,
) -> Report {
    Report {
        schema_version: 1,
        input: InputInfo {
            width: input.rgba.width(),
            height: input.rgba.height(),
            format: input.format.clone(),
        },
        output,
        template_id: Some(template.meta.id.clone()),
        template_version: Some(template.meta.version),
        quality_claim: Some(template.meta.quality),
        blend_mode: Some(template.meta.blend_mode),
        color_space: Some(template.meta.color_space),
        bbox: found.placement.map(bbox_of),
        shift: Some(found.shift),
        ncc: Some(found.ncc),
        alpha_mae: Some(found.alpha_mae),
        method: "inverse_blend".into(),
        verdict,
        flags,
        counts,
        holdout: None,
        camera_time: None,
    }
}

fn bbox_of(place: Placement) -> [i32; 4] {
    [
        place.origin_x.floor() as i32,
        place.origin_y.floor() as i32,
        place.width as i32,
        place.height as i32,
    ]
}

fn probe_from(found: &Match, place: &Placement) -> Probe {
    let alpha: Vec<u8> = found
        .alpha
        .iter()
        .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Probe {
        bbox: bbox_of(*place),
        unstable: vec![0; alpha.len()],
        alpha,
    }
}

fn invert(
    input: &InputImage,
    template: &LoadedTemplate,
    place: &Placement,
    alpha: &[f32],
    bias: &[f32],
) -> (RgbaImage, Counts, Coverage, Probe) {
    let x0 = place.origin_x.floor() as i32;
    let y0 = place.origin_y.floor() as i32;
    let w = place.width as i32;
    let h = place.height as i32;
    let pad = template.meta.placement.padding_px as i32;
    let stable_max = template.meta.alpha_stable_max;
    let mode = template.meta.blend_mode;
    let space = template.meta.color_space;
    let wm = template.meta.color;
    let use_bias = bias.len() == alpha.len() && !bias.is_empty();

    let (eval, ex, ey, ew, eh) = dilate(alpha, x0, y0, w, h, pad, input.rgba.width(), input.rgba.height());

    let mut out = input.rgba.clone();
    let mut core = 0u32;
    let mut unstable = 0u32;
    let mut clamped = 0u32;
    let mut unstable_sprite = vec![0u8; alpha.len()];

    let raw_in = input.rgba.as_raw();
    let raw_out = out.as_mut();
    let iw = input.rgba.width() as i32;

    for py in 0..h {
        for px in 0..w {
            let a = alpha[(py * w + px) as usize];
            if a <= 0.0 {
                continue;
            }
            let x = x0 + px;
            let y = y0 + py;
            if x < 0 || y < 0 || x >= iw || y >= input.rgba.height() as i32 {
                continue;
            }
            let exi = x - ex;
            let eyi = y - ey;
            let in_eval = exi >= 0
                && eyi >= 0
                && exi < ew as i32
                && eyi < eh as i32
                && eval[(eyi as u32 * ew + exi as u32) as usize];
            if !in_eval {
                continue;
            }
            let idx = ((y as u32 * input.rgba.width() + x as u32) * 4) as usize;
            let mut pixel_unstable = false;
            let mut pixel_clamped = false;
            if use_bias {
                let k = bias[(py * w + px) as usize];
                let m = 1.0 - a;
                if m < 1e-3 {
                    pixel_unstable = true;
                } else {
                    if a > stable_max {
                        pixel_unstable = true;
                    }
                    for c in 0..3 {
                        let src = (raw_in[idx + c] as f32 - k) / m;
                        if !src.is_finite() || src < 0.0 || src > 255.0 {
                            pixel_clamped = true;
                        }
                        if src.is_finite() {
                            raw_out[idx + c] = src.round().clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            } else {
                for c in 0..3 {
                    let inv = inverse_u8(raw_in[idx + c], a, wm[c], mode, space);
                    if inv.numeric_unstable {
                        pixel_unstable = true;
                        raw_out[idx + c] = raw_in[idx + c];
                    } else {
                        if a > stable_max {
                            pixel_unstable = true;
                        }
                        if inv.clamped {
                            pixel_clamped = true;
                        }
                        raw_out[idx + c] = inv.value;
                    }
                }
            }
            if a > 0.02 {
                core += 1;
                if pixel_unstable {
                    unstable += 1;
                    unstable_sprite[(py * w + px) as usize] = 1;
                }
                if pixel_clamped {
                    clamped += 1;
                }
            }
        }
    }

    let cov_alpha = coverage_alpha(&eval, ex, ey, ew, eh, alpha, x0, y0, w, h);
    let coverage = Coverage {
        x: ex,
        y: ey,
        width: ew,
        height: eh,
        alpha: cov_alpha,
        evaluate: eval,
    };
    let counts = Counts {
        core_pixels: core,
        unstable_pixels: unstable,
        clamped_pixels: clamped,
        outside_diff_pixels: 0,
    };
    let alpha_u8 = alpha
        .iter()
        .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let probe = Probe {
        bbox: [x0, y0, w, h],
        alpha: alpha_u8,
        unstable: unstable_sprite,
    };
    (out, counts, coverage, probe)
}

fn dilate(
    alpha: &[f32],
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    pad: i32,
    image_w: u32,
    image_h: u32,
) -> (Vec<bool>, i32, i32, u32, u32) {
    let x1 = (x0 - pad).max(0);
    let y1 = (y0 - pad).max(0);
    let x2 = (x0 + w + pad).min(image_w as i32);
    let y2 = (y0 + h + pad).min(image_h as i32);
    if x2 <= x1 || y2 <= y1 {
        return (Vec::new(), 0, 0, 0, 0);
    }
    let ew = (x2 - x1) as u32;
    let eh = (y2 - y1) as u32;
    let mut mask = vec![false; (ew * eh) as usize];
    for py in 0..h {
        for px in 0..w {
            if alpha[(py * w + px) as usize] <= 0.02 {
                continue;
            }
            let cx = x0 + px;
            let cy = y0 + py;
            for dy in -pad..=pad {
                for dx in -pad..=pad {
                    let x = cx + dx;
                    let y = cy + dy;
                    if x < x1 || y < y1 || x >= x2 || y >= y2 {
                        continue;
                    }
                    let ix = (x - x1) as u32;
                    let iy = (y - y1) as u32;
                    mask[(iy * ew + ix) as usize] = true;
                }
            }
        }
    }
    (mask, x1, y1, ew, eh)
}

fn coverage_alpha(
    eval: &[bool],
    ex: i32,
    ey: i32,
    ew: u32,
    eh: u32,
    alpha: &[f32],
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
) -> Vec<f32> {
    let mut out = vec![0.0f32; eval.len()];
    if ew == 0 || eh == 0 {
        return out;
    }
    for py in 0..h {
        for px in 0..w {
            let x = x0 + px;
            let y = y0 + py;
            let ix = x - ex;
            let iy = y - ey;
            if ix < 0 || iy < 0 || ix >= ew as i32 || iy >= eh as i32 {
                continue;
            }
            let i = (iy as u32 * ew + ix as u32) as usize;
            if eval.get(i) == Some(&true) {
                out[i] = alpha[(py * w + px) as usize];
            }
        }
    }
    out
}

pub fn outside_diff(original: &RgbaImage, output: &RgbaImage, coverage: &Coverage) -> u32 {
    if original.dimensions() != output.dimensions() {
        return u32::MAX;
    }
    let (w, h) = original.dimensions();
    let a = original.as_raw();
    let b = output.as_raw();
    let mut diff = 0u32;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let idx = ((y as u32 * w + x as u32) * 4) as usize;
            let ix = x - coverage.x;
            let iy = y - coverage.y;
            let in_eval = ix >= 0
                && iy >= 0
                && ix < coverage.width as i32
                && iy < coverage.height as i32
                && coverage.evaluate
                    .get((iy as u32 * coverage.width + ix as u32) as usize)
                    == Some(&true);
            if in_eval {
                if a[idx + 3] != b[idx + 3] {
                    diff += 1;
                }
            } else if a[idx..idx + 4] != b[idx..idx + 4] {
                diff += 1;
            }
        }
    }
    diff
}

fn apply_camera_time(
    input: &InputImage,
    opt: &ProcessOptions,
    mut output: ProcessOutput,
) -> Result<ProcessOutput, Error> {
    let Some(wanted) = opt.camera_time else {
        return Ok(output);
    };
    if !camera_time::is_valid_camera_time(wanted) {
        output.report.flags.push("camera_time_invalid".into());
        return Ok(output);
    }
    // Time editing does not wait for a watermark image. A held or failed
    // removal still rewrites the date on the original and returns that picture.
    // The date function returns before it paints when the stamp is missing,
    // so the taken image can be put back unchanged.
    let from_original = output.image.is_none();
    let mut image = output.image.take().unwrap_or_else(|| input.rgba.clone());
    match camera_time::edit_camera_time(&mut image, wanted) {
        Ok(change) => {
            output.encoded = Some(encode_result(&image, input, opt)?);
            output.report.output = Some(OutputInfo {
                format: if opt.lossy_jpeg { "jpeg".into() } else { "png".into() },
                lossy: opt.lossy_jpeg,
            });
            if matches!(
                output.report.verdict,
                Verdict::Pass | Verdict::PassWithUnstable | Verdict::NoWatermark | Verdict::Unsupported
            ) {
                output.report.verdict = Verdict::NeedsReview;
            }
            output.report.flags.push("camera_time_edited".into());
            if from_original && !opt.no_watermark {
                output.report.flags.push("watermark_not_written".into());
            }
            output.report.camera_time = Some(change);
            output.image = Some(image);
        }
        Err(CameraTimeError::NotFound) => {
            output.report.flags.push("camera_time_missing".into());
            keep_prior_image(&mut output, image, from_original, opt.no_watermark);
        }
        Err(CameraTimeError::Invalid) => {
            output.report.flags.push("camera_time_invalid".into());
            keep_prior_image(&mut output, image, from_original, opt.no_watermark);
        }
    }
    Ok(output)
}

fn keep_prior_image(output: &mut ProcessOutput, image: RgbaImage, from_original: bool, no_watermark: bool) {
    // A removed watermark is still a result when the date cannot be rewritten.
    // A plain copy is not: the user asked for a time change and did not get one.
    if !from_original && !no_watermark {
        output.image = Some(image);
        return;
    }
    output.image = None;
    output.encoded = None;
    output.report.output = None;
    if no_watermark {
        output.report.verdict = Verdict::NeedsReview;
    }
}

fn encode_result(image: &RgbaImage, input: &InputImage, opt: &ProcessOptions) -> Result<Vec<u8>, Error> {
    if opt.lossy_jpeg {
        return encode_jpeg(image, 95);
    }
    let mut png = encode_png(image)?;
    if opt.keep_exif {
        if let Some(tiff) = &input.exif_tiff {
            let mut patched = tiff.clone();
            if patch_exif_orientation_to_top_left(&mut patched) || !patched.is_empty() {
                png = png_insert_chunk(&png, b"eXIf", &patched)?;
            }
        }
    }
    Ok(png)
}

fn copy_through(input: &InputImage, opt: &ProcessOptions) -> Result<ProcessOutput, Error> {
    let encoded = encode_result(&input.rgba, input, opt)?;
    let report = Report {
        schema_version: 1,
        input: InputInfo {
            width: input.rgba.width(),
            height: input.rgba.height(),
            format: input.format.clone(),
        },
        output: Some(OutputInfo {
            format: if opt.lossy_jpeg { "jpeg".into() } else { "png".into() },
            lossy: opt.lossy_jpeg,
        }),
        template_id: None,
        template_version: None,
        quality_claim: None,
        blend_mode: None,
        color_space: None,
        bbox: None,
        shift: None,
        ncc: None,
        alpha_mae: None,
        method: "copy".into(),
        verdict: Verdict::NoWatermark,
        flags: {
            let mut f = Vec::new();
            if input.precision_reduced {
                f.push("precision_reduced".into());
            }
            if input.non_srgb {
                f.push("non_srgb".into());
            }
            if opt.lossy_jpeg {
                f.push("lossy_output".into());
            }
            f
        },
        counts: Counts::zero(),
        holdout: None,
        camera_time: None,
    };
    Ok(ProcessOutput {
        image: Some(input.rgba.clone()),
        encoded: Some(encoded),
        report,
        probe: None,
        coverage: None,
    })
}

/// Paste `alpha` with the template's blend and color space. Used by calibration
/// and by the synthetic fixture. Pixels with alpha 0 are left untouched.
pub fn composite_alpha(
    image: &mut RgbaImage,
    alpha: &[f32],
    origin_x: i32,
    origin_y: i32,
    width: u32,
    wm: [f32; 3],
    mode: BlendMode,
    space: ColorSpace,
) {
    let w = width as i32;
    let h = (alpha.len() / width as usize) as i32;
    for py in 0..h {
        for px in 0..w {
            let a = alpha[(py * w + px) as usize];
            if a <= 0.0 {
                continue;
            }
            let x = origin_x + px;
            let y = origin_y + py;
            if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
                continue;
            }
            let pxr = image.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                pxr.0[c] = crate::blend::forward_u8(pxr.0[c], a, wm[c], mode, space);
            }
        }
    }
}
