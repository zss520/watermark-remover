//! sRGB IEC 61966-2-1 transfer, used only when a template says the
//! platform composited in linear light.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    Srgb,
    Linear,
}

pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn to_work(encoded: f32, space: ColorSpace) -> f32 {
    match space {
        ColorSpace::Srgb => encoded.clamp(0.0, 1.0),
        ColorSpace::Linear => srgb_to_linear(encoded),
    }
}

pub fn from_work(working: f32, space: ColorSpace) -> f32 {
    match space {
        ColorSpace::Srgb => working,
        ColorSpace::Linear => linear_to_srgb(working),
    }
}
