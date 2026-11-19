//! Inverse-blend removal for known semi-transparent watermarks.
//!
//! Runtime reports never include PSNR or SSIM. Those exist only on holdout
//! pairs that still have a clean original.

#![forbid(unsafe_code)]

mod blend;
mod calibrate;
mod color;
mod image_io;
mod locate;
mod metrics;
mod placement;
mod report;
mod restore;
mod template;
mod verify;

pub mod fixture;

pub use blend::{forward_u8, forward_unit, inverse_pixel, inverse_u8, quantize_unit, BlendMode, InversePixel};
pub use calibrate::{calibrate_to_dir, CalibrateRequest, CalibrateSummary};
pub use color::ColorSpace;
pub use image_io::{decode_bytes, decode_path, encode_jpeg, encode_png, InputImage};
pub use locate::locate;
pub use placement::place_loaded as placement_of;
pub use report::{failed_when_outside_changes as verdict_if_outside, Counts, Report, Verdict};
pub use restore::{
    composite_alpha, outside_diff as restore_outside_for_test, process_best, process_one, ProcessOptions, ProcessOutput,
};
pub use template::{
    encode_alpha16, load_template_dir, load_template_embedded, load_template_embedded_parts, load_template_file,
    read_template_files, sha256_hex, LoadedTemplate, Quality, Template, TemplateFiles,
};
pub use verify::{verify_pairs, VerifyReport};

use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("无法读取文件：{0}")]
    Io(#[from] io::Error),
    #[error("图片无法解码：{0}")]
    Decode(String),
    #[error("不支持的图片格式")]
    UnsupportedFormat,
    #[error("{0}")]
    Template(String),
    #[error("Alpha 贴图与模板哈希不一致")]
    HashMismatch,
    #[error("模板字段无法识别：{0}")]
    Schema(String),
}
