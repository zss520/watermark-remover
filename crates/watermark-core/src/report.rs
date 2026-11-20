use crate::blend::BlendMode;
use crate::color::ColorSpace;
use crate::template::Quality;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    PassWithUnstable,
    NeedsReview,
    NoWatermark,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputInfo {
    pub width: u32,
    pub height: u32,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub format: String,
    pub lossy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub core_pixels: u32,
    pub unstable_pixels: u32,
    pub clamped_pixels: u32,
    pub outside_diff_pixels: u32,
}

impl Counts {
    pub fn zero() -> Self {
        Self {
            core_pixels: 0,
            unstable_pixels: 0,
            clamped_pixels: 0,
            outside_diff_pixels: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldoutMetrics {
    pub psnr: f64,
    pub ssim_min: f64,
    pub max_abs: u8,
    pub stable_max_abs: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTimeChange {
    pub original: String,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub input: InputInfo,
    pub output: Option<OutputInfo>,
    pub template_id: Option<String>,
    pub template_version: Option<u32>,
    pub quality_claim: Option<Quality>,
    pub blend_mode: Option<BlendMode>,
    pub color_space: Option<ColorSpace>,
    pub bbox: Option<[i32; 4]>,
    pub shift: Option<[i32; 2]>,
    pub ncc: Option<f32>,
    pub alpha_mae: Option<f32>,
    pub method: String,
    pub verdict: Verdict,
    pub flags: Vec<String>,
    pub counts: Counts,
    pub holdout: Option<HoldoutMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_time: Option<CameraTimeChange>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub ncc: f32,
    pub ncc_auto: f32,
    pub ncc_confirm: f32,
    pub applicable: bool,
    pub accept_low: bool,
    pub manual: bool,
    pub no_watermark: bool,
    pub quality_pixel: bool,
    pub core: u32,
    pub unstable: u32,
    pub clamped: u32,
    pub outside: u32,
    pub precision_reduced: bool,
    pub non_srgb: bool,
    pub lossy: bool,
    pub ambiguous: bool,
}

pub fn decide(d: &Decision) -> (Verdict, Vec<String>) {
    let mut flags = Vec::new();
    if d.precision_reduced {
        flags.push("precision_reduced".to_string());
    }
    if d.non_srgb {
        flags.push("non_srgb".to_string());
    }
    if d.lossy {
        flags.push("lossy_output".to_string());
    }
    if d.manual {
        flags.push("manual_bbox".to_string());
    }
    if d.ambiguous {
        flags.push("ambiguous_template".to_string());
    }

    if d.no_watermark {
        return (Verdict::NoWatermark, flags);
    }
    if d.outside > 0 {
        return (Verdict::Failed, flags);
    }
    if !d.applicable || d.ncc < d.ncc_confirm {
        return (Verdict::Unsupported, flags);
    }

    let low = d.ncc < d.ncc_auto;
    if d.ambiguous && !d.accept_low && !d.manual {
        flags.push("confirmation_required".to_string());
        return (Verdict::NeedsReview, flags);
    }
    if low && !d.accept_low && !d.manual {
        flags.push("confirmation_required".to_string());
        return (Verdict::NeedsReview, flags);
    }

    let ratio = if d.core == 0 {
        1.0
    } else {
        d.unstable as f32 / d.core as f32
    };
    let capped = d.precision_reduced
        || d.non_srgb
        || d.lossy
        || d.manual
        || low
        || d.ambiguous
        || !d.quality_pixel
        || d.core == 0
        || d.clamped > 0;

    if !capped && ratio <= 0.02 {
        return (Verdict::Pass, flags);
    }
    if !capped && ratio > 0.02 {
        return (Verdict::PassWithUnstable, flags);
    }
    (Verdict::NeedsReview, flags)
}

pub fn confirmation_required(flags: &[String]) -> bool {
    flags.iter().any(|f| f == "confirmation_required")
}

/// Decision used when a caller has already observed mask-exterior writes.
pub fn failed_when_outside_changes(diff: u32) -> Verdict {
    let d = Decision {
        ncc: 1.0,
        ncc_auto: 0.85,
        ncc_confirm: 0.60,
        applicable: true,
        accept_low: false,
        manual: false,
        no_watermark: false,
        quality_pixel: true,
        core: 10,
        unstable: 0,
        clamped: 0,
        outside: diff,
        precision_reduced: false,
        non_srgb: false,
        lossy: false,
        ambiguous: false,
    };
    decide(&d).0
}
