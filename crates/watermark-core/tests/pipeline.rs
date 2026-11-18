use image::Rgba;
use std::fs;
use watermark_core::fixture::{self, template_from_alpha};
use watermark_core::{
    decode_bytes, load_template_embedded, load_template_file, process_one, verify_pairs, CalibrateRequest,
    ColorSpace, Error, ProcessOptions, Quality, Verdict,
};
use watermark_core::calibrate_to_dir;

fn temp() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "wm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn synthetic_roundtrip_passes_and_outside_is_untouched() {
    let demo = fixture::demo();
    let input = watermark_core::InputImage {
        rgba: demo.watermarked.clone(),
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &demo.template, &ProcessOptions::default()).unwrap();
    assert_eq!(
        out.report.verdict,
        Verdict::Pass,
        "ncc={:?} mae={:?} flags={:?}",
        out.report.ncc,
        out.report.alpha_mae,
        out.report.flags
    );
    assert_eq!(out.report.counts.outside_diff_pixels, 0);
    assert_eq!(out.report.counts.clamped_pixels, 0);
    assert!(out.report.holdout.is_none());
    let restored = out.image.unwrap();
    let coverage = out.coverage.unwrap();
    assert_eq!(watermark_core::restore_outside_for_test(&demo.watermarked, &restored, &coverage), 0);

    let mut tampered = restored.clone();
    tampered.put_pixel(0, 0, Rgba([1, 2, 3, 255]));
    assert!(watermark_core::restore_outside_for_test(&demo.watermarked, &tampered, &coverage) > 0);

    let pair = vec![("a.png".into(), demo.watermarked, demo.clean)];
    let report = verify_pairs(&demo.template, &pair);
    assert!(report.items[0].passed, "{}", report.items[0].detail);
}

#[test]
fn outside_failure_is_a_failed_verdict() {
    let demo = fixture::demo();
    let input = watermark_core::InputImage {
        rgba: demo.watermarked,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &demo.template, &ProcessOptions::default()).unwrap();
    let coverage = out.coverage.unwrap();
    let mut tampered = out.image.unwrap();
    tampered.put_pixel(1, 1, Rgba([9, 9, 9, 255]));
    let diff = watermark_core::restore_outside_for_test(&input.rgba, &tampered, &coverage);
    assert!(diff > 0);
    let verdict = watermark_core::verdict_if_outside(diff);
    assert_eq!(verdict, Verdict::Failed);
}

#[test]
fn unstable_alpha_is_flagged() {
    let (w, h) = (320u32, 320u32);
    let aw = 64u32;
    let alpha = vec![0.85f32; (aw * aw) as usize];
    let template = template_from_alpha(alpha.clone(), aw, aw, w, h, Quality::Pixel);
    let mut image = fixture::scene(w, h);
    let placed = watermark_core::placement_of(&template, w, h).unwrap();
    watermark_core::composite_alpha(
        &mut image,
        &alpha,
        placed.origin_x.floor() as i32,
        placed.origin_y.floor() as i32,
        placed.width,
        [1.0, 1.0, 1.0],
        watermark_core::BlendMode::SourceOver,
        ColorSpace::Srgb,
    );
    let input = watermark_core::InputImage {
        rgba: image,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &template, &ProcessOptions::default()).unwrap();
    assert!(out.report.counts.unstable_pixels > 0);
    assert_ne!(out.report.verdict, Verdict::Pass);
    assert_eq!(out.report.verdict, Verdict::PassWithUnstable);
}

#[test]
fn locate_shift_negative() {
    let demo = fixture::demo();
    let placed = watermark_core::placement_of(&demo.template, 640, 480).unwrap();
    let mut image = demo.clean.clone();
    watermark_core::composite_alpha(
        &mut image,
        &demo.template.alpha,
        placed.origin_x.floor() as i32 - 16,
        placed.origin_y.floor() as i32,
        placed.width,
        [1.0, 1.0, 1.0],
        watermark_core::BlendMode::SourceOver,
        ColorSpace::Srgb,
    );
    let input = watermark_core::InputImage {
        rgba: image,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &demo.template, &ProcessOptions::default()).unwrap();
    assert!(out.report.ncc.unwrap_or(1.0) < 0.85, "ncc={}", out.report.ncc.unwrap());
    assert_ne!(out.report.verdict, Verdict::Pass);
    assert!(out.image.is_none());
}

#[test]
fn rgba_alpha_preserved() {
    let demo = fixture::demo();
    let mut marked = demo.watermarked;
    for px in marked.pixels_mut() {
        px.0[3] = 123;
    }
    let input = watermark_core::InputImage {
        rgba: marked,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &demo.template, &ProcessOptions::default()).unwrap();
    let image = out.image.expect("image");
    assert!(image.pixels().all(|p| p.0[3] == 123));
    assert_ne!(image.as_raw(), input.rgba.as_raw());
}

#[test]
fn template_hash_mismatch_and_unknown_field() {
    let demo = fixture::demo();
    let dir = temp();
    let png = watermark_core::encode_alpha16(&demo.template.alpha, 96, 48).unwrap();
    let hash = watermark_core::sha256_hex(&png);
    let mut meta = demo.template.meta.clone();
    meta.alpha_map = "alpha.png".into();
    meta.alpha_sha256 = hash;
    fs::write(dir.join("alpha.png"), &png).unwrap();
    fs::write(dir.join("t.json"), serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    assert!(load_template_file(&dir.join("t.json")).is_ok());
    let mut bad = png.clone();
    bad[40] ^= 0xff;
    fs::write(dir.join("alpha.png"), &bad).unwrap();
    let err = load_template_file(&dir.join("t.json")).unwrap_err();
    assert!(matches!(err, Error::HashMismatch));

    let mut value: serde_json::Value = serde_json::from_str(&serde_json::to_string(&meta).unwrap()).unwrap();
    value["extra"] = serde_json::json!(1);
    let err = load_template_embedded(&value.to_string(), &png).unwrap_err();
    assert!(matches!(err, Error::Schema(_)));
}

#[test]
fn jpeg_export_not_pixel() {
    let demo = fixture::demo();
    let input = watermark_core::InputImage {
        rgba: demo.watermarked,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let mut opt = ProcessOptions::default();
    opt.lossy_jpeg = true;
    let out = process_one(&input, &demo.template, &opt).unwrap();
    assert_ne!(out.report.verdict, Verdict::Pass);
    assert!(out.report.flags.iter().any(|f| f == "lossy_output"));
    assert_eq!(out.report.output.unwrap().format, "jpeg");
}

#[test]
fn holdout_gate_requires_ninety_five_percent() {
    let demo = fixture::demo();
    let mut pairs = Vec::new();
    for i in 0..4 {
        let d = fixture::demo();
        pairs.push((format!("{i}.png"), d.watermarked, d.clean));
    }
    let ok = verify_pairs(&demo.template, &pairs);
    assert!(ok.accepted, "{:?}", ok.items.iter().map(|i| &i.detail).collect::<Vec<_>>());

    pairs[3].1 = pairs[3].2.clone();
    let bad = verify_pairs(&demo.template, &pairs);
    assert!(!bad.accepted);
    assert!(bad.pass_rate < 0.95);
}

#[test]
fn screen_pipeline_roundtrip() {
    let (w, h) = (320u32, 320u32);
    let aw = 64u32;
    let mut alpha = vec![0.0f32; (aw * aw) as usize];
    for y in 8..56 {
        for x in 8..56 {
            alpha[(y * aw + x) as usize] = 0.20 + 0.35 * ((x - 8) as f32 / 48.0);
        }
    }
    let mut template = template_from_alpha(alpha.clone(), aw, aw, w, h, Quality::Pixel);
    template.meta.blend_mode = watermark_core::BlendMode::Screen;
    template.meta.color = [0.5, 0.5, 0.5];
    let placed = watermark_core::placement_of(&template, w, h).unwrap();
    let mut background = image::RgbaImage::from_pixel(w, h, Rgba([30, 30, 30, 255]));
    watermark_core::composite_alpha(
        &mut background,
        &alpha,
        placed.origin_x.floor() as i32,
        placed.origin_y.floor() as i32,
        placed.width,
        template.meta.color,
        template.meta.blend_mode,
        template.meta.color_space,
    );
    let input = watermark_core::InputImage {
        rgba: background,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &template, &ProcessOptions::default()).unwrap();
    assert_eq!(
        out.report.verdict,
        Verdict::Pass,
        "ncc={:?} mae={:?} flags={:?}",
        out.report.ncc,
        out.report.alpha_mae,
        out.report.flags
    );
}

#[test]
fn calibrate_distinguishes_srgb_and_linear() {
    let dir = temp();
    let black = fixture::plate(320, 320, [0, 0, 0], 0.40, 64, 64);
    let gray = fixture::plate(320, 320, [128, 128, 128], 0.40, 64, 64);
    let summary = calibrate_to_dir(
        &CalibrateRequest {
            id: "synth".into(),
            display_name: "合成".into(),
            blacks: vec![black],
            grays: vec![gray],
        },
        &dir,
    )
    .unwrap();
    assert!(summary.model_ok, "mae={} max={}", summary.gray_mae, summary.gray_max);
    assert_eq!(summary.color_space, "srgb");

    let dir2 = temp();
    let black = fixture::linear_plate(320, 320, 0, 0.40, 64, 64);
    let gray = fixture::linear_plate(320, 320, 128, 0.40, 64, 64);
    let summary = calibrate_to_dir(
        &CalibrateRequest {
            id: "lin".into(),
            display_name: "线性".into(),
            blacks: vec![black],
            grays: vec![gray],
        },
        &dir2,
    )
    .unwrap();
    assert!(summary.model_ok, "linear mae={} max={}", summary.gray_mae, summary.gray_max);
    assert_eq!(summary.color_space, "linear");
}

#[test]
fn confirmation_required_does_not_emit_pixels() {
    let demo = fixture::demo();
    let mut template = demo.template.clone();
    template.meta.ncc_auto = 1.01;
    template.meta.ncc_confirm = 0.2;
    let input = watermark_core::InputImage {
        rgba: demo.watermarked,
        format: "png".into(),
        precision_reduced: false,
        non_srgb: false,
        exif_tiff: None,
    };
    let out = process_one(&input, &template, &ProcessOptions::default()).unwrap();
    assert!(out.report.flags.iter().any(|f| f == "confirmation_required"));
    assert!(out.image.is_none());
    assert_eq!(out.report.verdict, Verdict::NeedsReview);
}

#[test]
fn bytes_roundtrip_png() {
    let demo = fixture::demo();
    let png = watermark_core::encode_png(&demo.watermarked).unwrap();
    let decoded = decode_bytes(&png).unwrap();
    assert_eq!(decoded.rgba.dimensions(), demo.watermarked.dimensions());
}
