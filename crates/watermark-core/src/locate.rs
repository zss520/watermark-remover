//! Confidence is normalized correlation between the template alpha and an
//! alpha implied by an independent background estimate. Round-trip residual
//! is not used: inverting with the same alpha always reconstructs the
//! observation, even when the template is wrong.

use crate::blend::BlendMode;
use crate::placement::{place_loaded, Placement};
use crate::template::{rasterize_alpha, LoadedTemplate};
use image::RgbaImage;

#[derive(Debug, Clone)]
pub struct Match {
    pub applicable: bool,
    pub ncc: f32,
    pub alpha_mae: f32,
    pub shift: [i32; 2],
    pub placement: Option<Placement>,
    pub alpha: Vec<f32>,
}

impl Match {
    pub fn none() -> Self {
        Self {
            applicable: false,
            ncc: 0.0,
            alpha_mae: 1.0,
            shift: [0, 0],
            placement: None,
            alpha: Vec::new(),
        }
    }
}

pub fn locate(image: &RgbaImage, template: &LoadedTemplate) -> Match {
    let Some(base) = place_loaded(template, image.width(), image.height()) else {
        return Match::none();
    };
    let (fx, fy) = base.frac();
    let alpha = rasterize_alpha(template, base.width, base.height, fx, fy);
    let mut best: Option<Match> = None;
    for dy in -4..=4 {
        for dx in -4..=4 {
            let placed = base.shifted(dx, dy);
            if !placed.fully_inside(image.width(), image.height()) {
                continue;
            }
            let (ncc, mae) = score(
                image,
                &alpha,
                placed,
                template.meta.color,
                template.meta.blend_mode,
            );
            let candidate = Match {
                applicable: true,
                ncc,
                alpha_mae: mae,
                shift: [dx, dy],
                placement: Some(placed),
                alpha: alpha.clone(),
            };
            best = Some(match best {
                None => candidate,
                Some(cur) => better(cur, candidate),
            });
        }
    }
    best.unwrap_or_else(Match::none)
}

fn better(a: Match, b: Match) -> Match {
    if b.ncc > a.ncc + 1e-4 {
        return b;
    }
    if (b.ncc - a.ncc).abs() <= 1e-4 && b.alpha_mae < a.alpha_mae {
        return b;
    }
    a
}

fn score(image: &RgbaImage, alpha: &[f32], place: Placement, wm: [f32; 3], mode: BlendMode) -> (f32, f32) {
    let x0 = place.origin_x.floor() as i32;
    let y0 = place.origin_y.floor() as i32;
    let w = place.width as i32;
    let h = place.height as i32;
    let bg = background(image, x0, y0, w, h);
    let mut obs = Vec::new();
    let mut tpl = Vec::new();
    for py in 0..h {
        for px in 0..w {
            let a = alpha[(py * w + px) as usize];
            if a <= 0.02 {
                continue;
            }
            let x = x0 + px;
            let y = y0 + py;
            if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
                continue;
            }
            let p = image.get_pixel(x as u32, y as u32).0;
            let mut acc = 0.0f32;
            let mut n = 0.0f32;
            for c in 0..3 {
                let b = bg[c][(py * w + px) as usize];
                if let Some(v) = implied_alpha(p[c] as f32 / 255.0, b, wm[c], mode) {
                    acc += v;
                    n += 1.0;
                }
            }
            if n == 0.0 {
                continue;
            }
            obs.push(acc / n);
            tpl.push(a);
        }
    }
    let mae = if obs.is_empty() {
        1.0
    } else {
        obs.iter()
            .zip(tpl.iter())
            .map(|(o, t)| (o - t).abs())
            .sum::<f32>()
            / obs.len() as f32
    };
    // A flat alpha has no variance, so correlation is undefined. A small
    // mean absolute error is then the confidence. Text-like templates keep
    // using NCC.
    let ncc = if obs.len() < 8 {
        0.0
    } else if variance(&tpl) < 1e-6 {
        (1.0 - mae / 0.08).clamp(0.0, 1.0)
    } else {
        ncc(&obs, &tpl)
    };
    (ncc, mae)
}

fn variance(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32
}

fn implied_alpha(out: f32, bg: f32, wm: f32, mode: BlendMode) -> Option<f32> {
    let value = match mode {
        BlendMode::SourceOver => {
            let denom = wm - bg;
            if denom.abs() <= 0.05 {
                return None;
            }
            (out - bg) / denom
        }
        BlendMode::Screen => {
            if (1.0 - bg).abs() <= 0.05 || wm.abs() <= 0.05 {
                return None;
            }
            let k = 1.0 - (1.0 - out) / (1.0 - bg);
            k / wm
        }
    };
    if value.is_finite() {
        Some(value.clamp(0.0, 1.0))
    } else {
        None
    }
}

fn ncc(a: &[f32], b: &[f32]) -> f32 {
    if a.len() < 8 || a.len() != b.len() {
        return 0.0;
    }
    let n = a.len() as f32;
    let ma = a.iter().sum::<f32>() / n;
    let mb = b.iter().sum::<f32>() / n;
    let mut num = 0.0f32;
    let mut da = 0.0f32;
    let mut db = 0.0f32;
    for i in 0..a.len() {
        let xa = a[i] - ma;
        let xb = b[i] - mb;
        num += xa * xb;
        da += xa * xa;
        db += xb * xb;
    }
    if da < 1e-8 || db < 1e-8 {
        return 0.0;
    }
    (num / (da.sqrt() * db.sqrt())).clamp(-1.0, 1.0)
}

/// Laplace fill of the sprite rectangle. The 4 px ring outside the rectangle
/// is the Dirichlet boundary. Initialized to the ring mean so a flat plate
/// is already the solution.
fn background(image: &RgbaImage, x0: i32, y0: i32, w: i32, h: i32) -> [Vec<f32>; 3] {
    let iw = image.width() as i32;
    let ih = image.height() as i32;
    let mut planes = [Vec::new(), Vec::new(), Vec::new()];
    for c in 0..3 {
        let mut sum = 0.0f32;
        let mut n = 0.0f32;
        for y in (y0 - 4)..(y0 + h + 4) {
            for x in (x0 - 4)..(x0 + w + 4) {
                if x < 0 || y < 0 || x >= iw || y >= ih {
                    continue;
                }
                let inside = x >= x0 && y >= y0 && x < x0 + w && y < y0 + h;
                if inside {
                    continue;
                }
                let dx = if x < x0 {
                    x0 - x
                } else if x >= x0 + w {
                    x - (x0 + w - 1)
                } else {
                    0
                };
                let dy = if y < y0 {
                    y0 - y
                } else if y >= y0 + h {
                    y - (y0 + h - 1)
                } else {
                    0
                };
                if dx.max(dy) >= 1 && dx.max(dy) <= 4 {
                    sum += image.get_pixel(x as u32, y as u32).0[c] as f32 / 255.0;
                    n += 1.0;
                }
            }
        }
        let mean = if n > 0.0 { sum / n } else { 0.0 };
        let sample = |x: i32, y: i32| -> f32 {
            if x >= 0 && y >= 0 && x < iw && y < ih {
                image.get_pixel(x as u32, y as u32).0[c] as f32 / 255.0
            } else {
                mean
            }
        };
        // Bilinear blend of the four edges matches any linear background,
        // which is the harmonic function those edges imply. SOR then
        // corrects texture that a single edge blend cannot represent.
        let mut buf = vec![mean; (w * h) as usize];
        for py in 0..h {
            for px in 0..w {
                let tx = if w > 1 { px as f32 / (w as f32 - 1.0) } else { 0.0 };
                let ty = if h > 1 { py as f32 / (h as f32 - 1.0) } else { 0.0 };
                let left = sample(x0 - 1, y0 + py);
                let right = sample(x0 + w, y0 + py);
                let top = sample(x0 + px, y0 - 1);
                let bottom = sample(x0 + px, y0 + h);
                let horiz = left * (1.0 - tx) + right * tx;
                let vert = top * (1.0 - ty) + bottom * ty;
                buf[(py * w + px) as usize] = 0.5 * (horiz + vert);
            }
        }
        let omega = 1.7f32;
        for _ in 0..40 {
            let mut max_delta = 0.0f32;
            for py in 0..h {
                for px in 0..w {
                    let mut acc = 0.0f32;
                    let mut cnt = 0.0f32;
                    for (nx, ny) in [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)] {
                        if nx >= 0 && ny >= 0 && nx < w && ny < h {
                            acc += buf[(ny * w + nx) as usize];
                        } else {
                            let ix = x0 + nx;
                            let iy = y0 + ny;
                            if ix >= 0 && iy >= 0 && ix < iw && iy < ih {
                                acc += image.get_pixel(ix as u32, iy as u32).0[c] as f32 / 255.0;
                            } else {
                                acc += mean;
                            }
                        }
                        cnt += 1.0;
                    }
                    let target = acc / cnt;
                    let idx = (py * w + px) as usize;
                    let old = buf[idx];
                    let newv = old + omega * (target - old);
                    max_delta = max_delta.max((newv - old).abs());
                    buf[idx] = newv;
                }
            }
            if max_delta < 1e-4 {
                break;
            }
        }
        planes[c] = buf;
    }
    planes
}

pub fn manual_match(template: &LoadedTemplate, image: &RgbaImage, bbox: [u32; 4]) -> Match {
    let [x, y, w, h] = bbox;
    if w == 0 || h == 0 {
        return Match::none();
    }
    let place = Placement {
        origin_x: x as f32,
        origin_y: y as f32,
        width: w,
        height: h,
    };
    if !place.fully_inside(image.width(), image.height()) {
        return Match::none();
    }
    let alpha = rasterize_alpha(template, w, h, 0.0, 0.0);
    let (ncc, mae) = score(image, &alpha, place, template.meta.color, template.meta.blend_mode);
    Match {
        applicable: true,
        ncc,
        alpha_mae: mae,
        shift: [0, 0],
        placement: Some(place),
        alpha,
    }
}
