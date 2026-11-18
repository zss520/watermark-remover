//! Source-over and Screen, dispatched separately.
//! When the watermark is pure white the two forward formulas match;
//! a non-white vector is what proves the Screen branch is not the other one.

use crate::color::{from_work, to_work, ColorSpace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[serde(rename = "source_over")]
    SourceOver,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InversePixel {
    Value(f32),
    NumericUnstable,
}

/// `1 - α·C_wm < 1e-3`, and the actual denominator, both refuse the divide.
pub fn inverse_pixel(out: f32, alpha: f32, wm: f32, mode: BlendMode) -> InversePixel {
    if !out.is_finite() || !alpha.is_finite() || !wm.is_finite() {
        return InversePixel::NumericUnstable;
    }
    let k = alpha * wm;
    if 1.0 - k < 1e-3 {
        return InversePixel::NumericUnstable;
    }
    let src = match mode {
        BlendMode::SourceOver => {
            let denom = 1.0 - alpha;
            if denom < 1e-3 {
                return InversePixel::NumericUnstable;
            }
            (out - alpha * wm) / denom
        }
        BlendMode::Screen => {
            let denom = 1.0 - k;
            if denom < 1e-3 {
                return InversePixel::NumericUnstable;
            }
            (out - k) / denom
        }
    };
    if src.is_finite() {
        InversePixel::Value(src)
    } else {
        InversePixel::NumericUnstable
    }
}

pub fn forward_unit(src: f32, alpha: f32, wm: f32, mode: BlendMode) -> f32 {
    match mode {
        BlendMode::SourceOver => alpha * wm + (1.0 - alpha) * src,
        BlendMode::Screen => {
            let k = alpha * wm;
            1.0 - (1.0 - src) * (1.0 - k)
        }
    }
}

/// Half away from zero, then clamp into 0..=255. `clamped` is about the
/// unit value, before quantization.
pub fn quantize_unit(unit: f32) -> (u8, bool) {
    let clamped = !unit.is_finite() || unit < 0.0 || unit > 1.0;
    let scaled = if unit.is_finite() { unit * 255.0 } else { 0.0 };
    let rounded = scaled.round();
    let byte = rounded.clamp(0.0, 255.0);
    (byte as u8, clamped)
}

pub fn forward_u8(src: u8, alpha: f32, wm: f32, mode: BlendMode, space: ColorSpace) -> u8 {
    let src_w = to_work(src as f32 / 255.0, space);
    let wm_w = to_work(wm, space);
    let out_w = forward_unit(src_w, alpha, wm_w, mode).clamp(0.0, 1.0);
    quantize_unit(from_work(out_w, space)).0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInverse {
    pub value: u8,
    pub numeric_unstable: bool,
    pub clamped: bool,
}

pub fn inverse_u8(
    out: u8,
    alpha: f32,
    wm: f32,
    mode: BlendMode,
    space: ColorSpace,
) -> ChannelInverse {
    if alpha <= 1e-6 {
        return ChannelInverse {
            value: out,
            numeric_unstable: false,
            clamped: false,
        };
    }
    let out_w = to_work(out as f32 / 255.0, space);
    let wm_w = to_work(wm, space);
    match inverse_pixel(out_w, alpha, wm_w, mode) {
        InversePixel::NumericUnstable => ChannelInverse {
            value: out,
            numeric_unstable: true,
            clamped: false,
        },
        InversePixel::Value(src_w) => {
            let clamped = !src_w.is_finite() || src_w < 0.0 || src_w > 1.0;
            let encoded = from_work(src_w.clamp(0.0, 1.0), space);
            let (value, qclamp) = quantize_unit(encoded);
            ChannelInverse {
                value,
                numeric_unstable: false,
                clamped: clamped || qclamp,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_abs_white(mode_a: BlendMode, mode_b: BlendMode) -> f32 {
        let mut max = 0.0f32;
        for s in 0..256 {
            for step in 0..100 {
                let a = step as f32 / 100.0;
                let d = (forward_unit(s as f32 / 255.0, a, 1.0, mode_a)
                    - forward_unit(s as f32 / 255.0, a, 1.0, mode_b))
                .abs();
                max = max.max(d);
            }
        }
        max
    }

    #[test]
    fn inverse_matches_forward_white() {
        assert!(max_abs_white(BlendMode::SourceOver, BlendMode::Screen) < 1e-6);
        let out = forward_unit(0.2, 0.4, 1.0, BlendMode::SourceOver);
        assert!((out - 0.52).abs() < 1e-6);
        assert!((forward_unit(0.2, 0.4, 1.0, BlendMode::Screen) - 0.52).abs() < 1e-6);
        match inverse_pixel(out, 0.4, 1.0, BlendMode::SourceOver) {
            InversePixel::Value(v) => assert!((v - 0.2).abs() < 1e-6),
            InversePixel::NumericUnstable => panic!("stable alpha marked unstable"),
        }
        match inverse_pixel(out, 0.4, 1.0, BlendMode::Screen) {
            InversePixel::Value(v) => assert!((v - 0.2).abs() < 1e-6),
            InversePixel::NumericUnstable => panic!("stable alpha marked unstable"),
        }
    }

    #[test]
    fn screen_differs_when_color_not_white() {
        let screen = forward_unit(0.2, 0.4, 0.5, BlendMode::Screen);
        let over = forward_unit(0.2, 0.4, 0.5, BlendMode::SourceOver);
        assert!((screen - 0.36).abs() < 1e-5);
        assert!((over - 0.32).abs() < 1e-5);
        assert!((screen - over).abs() > 0.03);
        let wide = (forward_unit(1.0, 0.9, 0.0, BlendMode::Screen)
            - forward_unit(1.0, 0.9, 0.0, BlendMode::SourceOver))
        .abs();
        assert!(wide > 0.1, "wide={wide}");
        match inverse_pixel(screen, 0.4, 0.5, BlendMode::Screen) {
            InversePixel::Value(v) => assert!((v - 0.2).abs() < 1e-5),
            InversePixel::NumericUnstable => panic!("expected a value"),
        }
        // The source-over inverse of a Screen result is the wrong branch.
        let wrong = (0.36 - 0.4 * 0.5) / (1.0 - 0.4);
        assert!((wrong - 0.26666668_f32).abs() < 1e-4);
    }

    #[test]
    fn round_half_away_from_zero() {
        assert_eq!(177.5f32.round() as u8, 178);
        let out = forward_u8(100, 0.5, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
        assert_eq!(out, 178);
        let back = inverse_u8(out, 0.5, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
        assert!(!back.numeric_unstable);
        assert!(!back.clamped);
        assert_eq!(back.value, 101);
        assert_eq!(back.value.abs_diff(100), 1);
    }

    #[test]
    fn stable_alpha_exhaustive() {
        // max abs, mean, count of source values with error > 2
        let expect = [
            (0.20, 1u8, 0.199, 0u32),
            (0.40, 1, 0.398, 0),
            (0.50, 1, 0.500, 0),
            (0.60, 1, 0.598, 0),
            (0.70, 2, 0.801, 0),
            (0.75, 2, 0.996, 0),
            (0.80, 2, 1.195, 0),
        ];
        for (alpha, max_e, mean_e, gt2) in expect {
            let mut max = 0u8;
            let mut sum = 0u32;
            let mut over = 0u32;
            for s in 0..256u16 {
                let out = forward_u8(s as u8, alpha, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
                let back = inverse_u8(out, alpha, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
                assert!(!back.numeric_unstable, "alpha {alpha} src {s}");
                let err = back.value.abs_diff(s as u8);
                max = max.max(err);
                sum += err as u32;
                if err > 2 {
                    over += 1;
                }
            }
            let mean = sum as f32 / 256.0;
            assert_eq!(max, max_e, "max at alpha {alpha}");
            assert!((mean - mean_e).abs() < 0.02, "mean {mean} vs {mean_e} at {alpha}");
            assert_eq!(over, gt2, "gt2 at alpha {alpha}");
        }
    }

    #[test]
    fn unstable_boundary_matches_table() {
        let alpha = 0.85f32;
        let mut max = 0u8;
        let mut over = 0u32;
        for s in 0..256u16 {
            let out = forward_u8(s as u8, alpha, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
            let back = inverse_u8(out, alpha, 1.0, BlendMode::SourceOver, ColorSpace::Srgb);
            let err = back.value.abs_diff(s as u8);
            max = max.max(err);
            if err > 2 {
                over += 1;
            }
        }
        assert_eq!(max, 3);
        assert!(over >= 60, "over={over}");
        assert!(matches!(
            inverse_pixel(0.5, 0.9995, 1.0, BlendMode::SourceOver),
            InversePixel::NumericUnstable
        ));
    }
}
