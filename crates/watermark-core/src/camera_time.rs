//! Optional rewrite of a camera date stamp such as `2026/11/15 16:08`
//! or `2026.11.05 09:12`.
//!
//! The glyphs are redrawn with a bundled Inter SemiBold subset. It is the
//! closest freely licensed match to the Huawei stamp measured on the sample
//! photos; it is not the proprietary camera font. Size follows the detected
//! digit height, and the separator and the presence of seconds follow the
//! stamp that is already on the picture.

use crate::report::CameraTimeChange;
use fontdue::{Font, FontSettings};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTimeValue {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCameraTime {
    pub text: String,
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: Option<u8>,
    pub separator: char,
}

#[derive(Debug)]
pub enum CameraTimeError {
    NotFound,
    Invalid,
}

#[derive(Clone, Copy)]
struct Comp {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    area: u32,
}

struct Stamp {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    height: i32,
    separator: char,
    has_second: bool,
    text: String,
}

pub fn detect_camera_time(image: &RgbaImage) -> Option<DetectedCameraTime> {
    let stamp = find_stamp(image)?;
    let parsed = parse_stamp(&stamp.text)?;
    Some(DetectedCameraTime {
        text: stamp.text,
        year: parsed.0,
        month: parsed.1,
        day: parsed.2,
        hour: parsed.3,
        minute: parsed.4,
        second: parsed.5,
        separator: stamp.separator,
    })
}

pub fn is_valid_camera_time(value: CameraTimeValue) -> bool {
    valid_clock(value.hour, value.minute, value.second) && valid_date(value.year, value.month, value.day)
}

pub fn edit_camera_time(image: &mut RgbaImage, value: CameraTimeValue) -> Result<CameraTimeChange, CameraTimeError> {
    if !is_valid_camera_time(value) {
        return Err(CameraTimeError::Invalid);
    }
    let stamp = find_stamp(image).ok_or(CameraTimeError::NotFound)?;
    let second = if stamp.has_second {
        value.second.or_else(|| parse_stamp(&stamp.text).and_then(|p| p.5))
    } else {
        None
    };
    let updated = format_stamp(stamp.separator, stamp.has_second, value.year, value.month, value.day, value.hour, value.minute, second);
    let color = ink_color(image, stamp.x0, stamp.y0, stamp.x1, stamp.y1);
    let (bg, cutoff) = background(image, stamp.x0, stamp.y0, stamp.x1, stamp.y1);
    erase_and_inpaint(image, stamp.x0, stamp.y0, stamp.x1, stamp.y1, bg, cutoff);
    let px = px_for_height(stamp.height.max(1) as u32);
    let cx = (stamp.x0 + stamp.x1) / 2;
    let cy = (stamp.y0 + stamp.y1) / 2;
    draw_text(image, &updated, cx, cy, px, color);
    Ok(CameraTimeChange {
        original: stamp.text,
        updated,
    })
}

fn font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        Font::from_bytes(
            include_bytes!("../fonts/Inter-SemiBold.ttf") as &[u8],
            FontSettings::default(),
        )
        .ok()
    })
    .as_ref()
}

fn find_stamp(image: &RgbaImage) -> Option<Stamp> {
    let thresholds = [150, 185];
    let contrasts = [62, 48];
    for (&luma_min, &contrast) in thresholds.iter().zip(contrasts.iter()) {
        if let Some(stamp) = find_at(image, luma_min, contrast, 0) {
            return Some(stamp);
        }
    }
    // White type on a light wall is only a little brighter than the plaster,
    // and a thin stroke drops out at the stricter contrasts. Camera stamps
    // sit along the bottom, so this pass stays in that band.
    let h = image.height() as i32;
    let top = (h - (h / 5).max(80)).max(0);
    find_at(image, 150, 28, top)
}

fn find_at(image: &RgbaImage, luma_min: i32, contrast: i32, y_start: i32) -> Option<Stamp> {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if w < 16 || h < 16 {
        return None;
    }
    let mut mask = vec![0u8; (w * h) as usize];
    for y in y_start..h {
        for x in 0..w {
            let lum = luma_at(image, x, y);
            if lum < luma_min {
                continue;
            }
            let bg = darkest_ring(image, x, y, w, h);
            if lum - bg >= contrast {
                mask[(y * w + x) as usize] = 1;
            }
        }
    }
    let comps = components(&mask, w, h);
    let mut lines = group_lines(&comps);
    lines.sort_by_key(|line| std::cmp::Reverse(line_bounds(&comps, line).3));
    for line in lines {
        // The model name and the date share a baseline. A wide gap keeps the
        // date from being swallowed by the longer model line.
        for part in split_wide_gaps(&comps, &line) {
            if let Some(stamp) = read_line(&mask, w, &comps, &part) {
                return Some(stamp);
            }
        }
    }
    None
}

fn darkest_ring(image: &RgbaImage, x: i32, y: i32, w: i32, h: i32) -> i32 {
    const RING: [(i32, i32); 8] = [(8, 0), (-8, 0), (0, 8), (0, -8), (6, 6), (6, -6), (-6, 6), (-6, -6)];
    let mut samples = [255i32; 8];
    let mut n = 0;
    for (dx, dy) in RING {
        let xx = x + dx;
        let yy = y + dy;
        if xx >= 0 && yy >= 0 && xx < w && yy < h {
            samples[n] = luma_at(image, xx, yy);
            n += 1;
        }
    }
    if n < 5 {
        return 255;
    }
    samples[..n].sort_unstable();
    (samples[0] + samples[1] + samples[2]) / 3
}

fn components(mask: &[u8], w: i32, h: i32) -> Vec<Comp> {
    let mut seen = vec![false; mask.len()];
    let mut comps = Vec::new();
    let mut stack = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let start = (y * w + x) as usize;
            if mask[start] == 0 || seen[start] {
                continue;
            }
            let mut x0 = x;
            let mut y0 = y;
            let mut x1 = x;
            let mut y1 = y;
            let mut area = 0u32;
            stack.push((x, y));
            seen[start] = true;
            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                x0 = x0.min(cx);
                y0 = y0.min(cy);
                x1 = x1.max(cx);
                y1 = y1.max(cy);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let nx = cx + dx;
                    let ny = cy + dy;
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let ni = (ny * w + nx) as usize;
                    if mask[ni] == 0 || seen[ni] {
                        continue;
                    }
                    seen[ni] = true;
                    stack.push((nx, ny));
                }
            }
            let bw = x1 - x0 + 1;
            let bh = y1 - y0 + 1;
            if bh < 2 || bw > 80 || bh > 80 || area < 2 || area > 4000 {
                continue;
            }
            comps.push(Comp { x0, y0, x1, y1, area });
        }
    }
    comps
}

fn group_lines(comps: &[Comp]) -> Vec<Vec<usize>> {
    let mut body: Vec<usize> = comps
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let h = c.y1 - c.y0 + 1;
            let w = c.x1 - c.x0 + 1;
            h >= 6 && w <= 70 && c.area >= 8
        })
        .map(|(i, _)| i)
        .collect();
    body.sort_by_key(|&i| (comps[i].y0 + comps[i].y1) / 2);
    let mut lines: Vec<Vec<usize>> = Vec::new();
    for idx in body {
        let cy = (comps[idx].y0 + comps[idx].y1) / 2;
        let mut placed = false;
        for line in &mut lines {
            let (_, y0, _, y1) = line_bounds(comps, line);
            let mid = (y0 + y1) / 2;
            let h = (y1 - y0).max(1);
            if (cy - mid).abs() * 2 <= h {
                line.push(idx);
                placed = true;
                break;
            }
        }
        if !placed {
            lines.push(vec![idx]);
        }
    }
    lines
}

fn split_wide_gaps(comps: &[Comp], line: &[usize]) -> Vec<Vec<usize>> {
    let mut idxs = line.to_vec();
    idxs.sort_by_key(|&i| comps[i].x0);
    if idxs.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<i32> = idxs.iter().map(|&i| comps[i].y1 - comps[i].y0 + 1).collect();
    heights.sort_unstable();
    let upper = &heights[heights.len() / 2..];
    let typical = upper[upper.len() / 2].max(1);
    let mut groups = Vec::new();
    let mut current = vec![idxs[0]];
    for &idx in idxs.iter().skip(1) {
        let prev = comps[*current.last().unwrap()];
        let gap = comps[idx].x0 - prev.x1 - 1;
        if gap > typical * 3 {
            groups.push(std::mem::take(&mut current));
        }
        current.push(idx);
    }
    groups.push(current);
    groups
}

fn merge_stacked(comps: Vec<Comp>) -> Vec<Comp> {
    let mut out: Vec<Comp> = Vec::new();
    for comp in comps {
        if let Some(prev) = out.last_mut() {
            let overlap = prev.x1.min(comp.x1) - prev.x0.max(comp.x0) + 1;
            let narrow = (prev.x1 - prev.x0 + 1).min(comp.x1 - comp.x0 + 1);
            let vgap = if comp.y0 > prev.y1 {
                comp.y0 - prev.y1 - 1
            } else if prev.y0 > comp.y1 {
                prev.y0 - comp.y1 - 1
            } else {
                0
            };
            // A broken digit stacks two wide pieces. A separator dot is narrower
            // than this and stays in the gap.
            if overlap > 0 && overlap * 2 >= narrow && narrow >= 10 && vgap <= 4 {
                prev.x0 = prev.x0.min(comp.x0);
                prev.y0 = prev.y0.min(comp.y0);
                prev.x1 = prev.x1.max(comp.x1);
                prev.y1 = prev.y1.max(comp.y1);
                prev.area = prev.area.saturating_add(comp.area);
                continue;
            }
        }
        out.push(comp);
    }
    out
}

fn line_bounds(comps: &[Comp], line: &[usize]) -> (i32, i32, i32, i32) {
    let mut x0 = i32::MAX;
    let mut y0 = i32::MAX;
    let mut x1 = i32::MIN;
    let mut y1 = i32::MIN;
    for &i in line {
        x0 = x0.min(comps[i].x0);
        y0 = y0.min(comps[i].y0);
        x1 = x1.max(comps[i].x1);
        y1 = y1.max(comps[i].y1);
    }
    (x0, y0, x1, y1)
}

fn read_line(mask: &[u8], w: i32, comps: &[Comp], line: &[usize]) -> Option<Stamp> {
    // Dots and colon marks sit on the same line and are dropped after the
    // height filter, so the raw count is wider than the digit count.
    if !(8..=32).contains(&line.len()) {
        return None;
    }
    let mut parts: Vec<Comp> = line.iter().map(|&i| comps[i]).collect();
    parts.sort_by_key(|comp| (comp.x0, comp.y0));
    let parts = merge_stacked(parts);
    let mut heights: Vec<i32> = parts.iter().map(|comp| comp.y1 - comp.y0 + 1).collect();
    heights.sort_unstable();
    let median_h = heights[heights.len() / 2];
    if median_h < 6 {
        return None;
    }
    let kept: Vec<Comp> = parts
        .into_iter()
        .filter(|comp| {
            let h = comp.y1 - comp.y0 + 1;
            h * 10 >= median_h * 6 && h * 10 <= median_h * 14
        })
        .collect();
    if !(10..=16).contains(&kept.len()) {
        return None;
    }
    let px = px_for_height(median_h as u32);
    let glyphs = glyph_bitmaps(px)?;
    let x0 = kept.iter().map(|comp| comp.x0).min().unwrap_or(0);
    let y0 = kept.iter().map(|comp| comp.y0).min().unwrap_or(0);
    let x1 = kept.iter().map(|comp| comp.x1).max().unwrap_or(0);
    let y1 = kept.iter().map(|comp| comp.y1).max().unwrap_or(0);
    let mut gaps = Vec::with_capacity(kept.len());
    for n in 1..kept.len() {
        let prev = kept[n - 1];
        let cur = kept[n];
        let gap = cur.x0 - prev.x1 - 1;
        gaps.push(if gap > 1 {
            Some(inspect_gap(mask, w, prev.x1 + 1, cur.x0 - 1, y0, y1))
        } else {
            None
        });
    }
    let mut empty_widths: Vec<i32> = gaps.iter().filter_map(|gap| gap.as_ref()).filter(|gap| gap.ink < 3).map(|gap| gap.width).collect();
    empty_widths.sort_unstable();
    // Narrow digits such as "1" leave a gap that is almost as wide as a word
    // space. When one empty gap is clearly wider than the others, only that
    // one is the space.
    let space_at = if empty_widths.len() >= 2 {
        let largest = empty_widths[empty_widths.len() - 1];
        let next = empty_widths[empty_widths.len() - 2];
        if largest * 10 >= next * 14 { next + 1 } else { -1 }
    } else {
        -1
    };
    let mut text = String::new();
    for (n, comp) in kept.iter().enumerate() {
        if n > 0 {
            if let Some(gap) = &gaps[n - 1] {
                match gap_char(gap, y0, y1, median_h, space_at) {
                    Some('\0') => {}
                    Some(ch) => text.push(ch),
                    None => return None,
                }
            }
        }
        let ch = classify(mask, w, *comp, &glyphs)?;
        text.push(ch);
    }
    let parsed = parse_stamp(&text)?;
    Some(Stamp {
        x0,
        y0,
        x1,
        y1,
        height: median_h,
        separator: parsed.6,
        has_second: parsed.5.is_some(),
        text,
    })
}

struct GapInk {
    width: i32,
    ink: u32,
    runs: u32,
    gh: i32,
    gw: i32,
    mid: i32,
}

fn inspect_gap(mask: &[u8], w: i32, x0: i32, x1: i32, y0: i32, y1: i32) -> GapInk {
    let mut ink = 0u32;
    let mut ix0 = i32::MAX;
    let mut iy0 = i32::MAX;
    let mut ix1 = i32::MIN;
    let mut iy1 = i32::MIN;
    let mut rows = Vec::new();
    for y in y0..=y1 {
        let mut on = false;
        for x in x0..=x1 {
            if mask[(y * w + x) as usize] != 0 {
                on = true;
                ink += 1;
                ix0 = ix0.min(x);
                iy0 = iy0.min(y);
                ix1 = ix1.max(x);
                iy1 = iy1.max(y);
            }
        }
        rows.push(on);
    }
    let mut runs = 0u32;
    let mut prev = false;
    for on in rows {
        if on && !prev {
            runs += 1;
        }
        prev = on;
    }
    GapInk {
        width: x1 - x0 + 1,
        ink,
        runs,
        gh: if ink == 0 { 0 } else { iy1 - iy0 + 1 },
        gw: if ink == 0 { 0 } else { ix1 - ix0 + 1 },
        mid: if ink == 0 { 0 } else { (iy0 + iy1) / 2 },
    }
}

fn gap_char(gap: &GapInk, y0: i32, y1: i32, line_h: i32, space_at: i32) -> Option<char> {
    let wide = if space_at >= 0 {
        gap.width >= space_at
    } else {
        gap.width * 10 > line_h * 3
    };
    if gap.ink < 3 {
        return if wide { Some(' ') } else { Some('\0') };
    }
    if gap.runs == 2 {
        return Some(':');
    }
    if gap.runs == 1 {
        let line_mid = (y0 + y1) / 2;
        let centered = (gap.mid - line_mid).abs() * 5 <= line_h;
        let low = gap.mid >= y0 + line_h * 3 / 5;
        if gap.gh * 10 <= line_h * 5 && gap.gw >= gap.gh.saturating_mul(2) && centered {
            return Some('-');
        }
        if gap.gh * 10 <= line_h * 6 && gap.gw * 10 <= gap.gh * 18 && low {
            return Some('.');
        }
    }
    if gap.gh * 3 <= line_h && gap.gw * 3 <= line_h {
        return Some('\0');
    }
    if wide { None } else { Some('\0') }
}

struct GlyphBmp {
    ch: char,
    width: usize,
    height: usize,
    bitmap: Vec<u8>,
}

fn glyph_bitmaps(px: f32) -> Option<Vec<GlyphBmp>> {
    let font = font()?;
    let mut out = Vec::new();
    for ch in ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '/'] {
        let (metrics, bitmap) = font.rasterize(ch, px);
        if metrics.width == 0 || metrics.height == 0 {
            return None;
        }
        out.push(GlyphBmp {
            ch,
            width: metrics.width,
            height: metrics.height,
            bitmap,
        });
    }
    Some(out)
}

fn classify(mask: &[u8], w: i32, comp: Comp, glyphs: &[GlyphBmp]) -> Option<char> {
    let (ch, score) = classify_scored(mask, w, comp, glyphs);
    if score >= 0.38 { Some(ch) } else { None }
}

fn classify_scored(mask: &[u8], w: i32, comp: Comp, glyphs: &[GlyphBmp]) -> (char, f32) {
    let mut best = 0.0f32;
    let mut ch = '?';
    for glyph in glyphs {
        let score = iou(mask, w, comp, glyph);
        if score > best {
            best = score;
            ch = glyph.ch;
        }
    }
    if ch == '/' && looks_like_slash(mask, w, comp) {
        return ('/', best.max(0.45));
    }
    (ch, best)
}

fn iou(mask: &[u8], w: i32, comp: Comp, glyph: &GlyphBmp) -> f32 {
    let dw = (comp.x1 - comp.x0 + 1) as usize;
    let dh = (comp.y1 - comp.y0 + 1) as usize;
    if dw == 0 || dh == 0 || glyph.width == 0 || glyph.height == 0 {
        return 0.0;
    }
    let scaled_w = ((glyph.width * dh) as f32 / glyph.height as f32).round().max(1.0) as usize;
    let canvas = dw.max(scaled_w);
    let src_off = (canvas - dw) / 2;
    let font_off = (canvas - scaled_w) / 2;
    let mut inter = 0u32;
    let mut union = 0u32;
    for y in 0..dh {
        let sy = y * glyph.height / dh;
        for x in 0..canvas {
            let src_on = if x >= src_off && x < src_off + dw {
                mask[((comp.y0 as usize + y) * w as usize) + comp.x0 as usize + (x - src_off)] != 0
            } else {
                false
            };
            let font_on = if x >= font_off && x < font_off + scaled_w {
                let sx = (x - font_off) * glyph.width / scaled_w;
                glyph.bitmap[sy * glyph.width + sx] > 140
            } else {
                false
            };
            if font_on && src_on {
                inter += 1;
            }
            if font_on || src_on {
                union += 1;
            }
        }
    }
    if union == 0 { 0.0 } else { inter as f32 / union as f32 }
}

fn looks_like_slash(mask: &[u8], w: i32, comp: Comp) -> bool {
    let mut n = 0.0f32;
    let mut sx = 0.0f32;
    let mut sy = 0.0f32;
    let mut syy = 0.0f32;
    let mut sxy = 0.0f32;
    for y in comp.y0..=comp.y1 {
        for x in comp.x0..=comp.x1 {
            if mask[(y * w + x) as usize] == 0 {
                continue;
            }
            let xf = (x - comp.x0) as f32;
            let yf = (y - comp.y0) as f32;
            n += 1.0;
            sx += xf;
            sy += yf;
            syy += yf * yf;
            sxy += xf * yf;
        }
    }
    if n < 8.0 {
        return false;
    }
    let var_y = syy / n - (sy / n) * (sy / n);
    if var_y < 4.0 {
        return false;
    }
    let slope = (sxy / n - (sx / n) * (sy / n)) / var_y;
    let width = (comp.x1 - comp.x0 + 1) as f32;
    let height = (comp.y1 - comp.y0 + 1) as f32;
    slope < -0.12 && slope > -1.4 && width < height * 0.85
}

fn parse_stamp(text: &str) -> Option<(u16, u8, u8, u8, u8, Option<u8>, char)> {
    let b = text.as_bytes();
    if b.len() != 16 && b.len() != 19 {
        return None;
    }
    let sep = b[4] as char;
    if sep != '/' && sep != '-' && sep != '.' || b[7] != b[4] || b[10] != b' ' || b[13] != b':' {
        return None;
    }
    if b.len() == 19 && b[16] != b':' {
        return None;
    }
    let year = parse_digits(&b[0..4])? as u16;
    let month = parse_digits(&b[5..7])? as u8;
    let day = parse_digits(&b[8..10])? as u8;
    let hour = parse_digits(&b[11..13])? as u8;
    let minute = parse_digits(&b[14..16])? as u8;
    let second = if b.len() == 19 {
        Some(parse_digits(&b[17..19])? as u8)
    } else {
        None
    };
    if !valid_date(year, month, day) || !valid_clock(hour, minute, second) {
        return None;
    }
    Some((year, month, day, hour, minute, second, sep))
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    let mut n = 0u32;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + (b - b'0') as u32;
    }
    Some(n)
}

fn valid_clock(hour: u8, minute: u8, second: Option<u8>) -> bool {
    hour <= 23 && minute <= 59 && second.map(|s| s <= 59).unwrap_or(true)
}

fn valid_date(year: u16, month: u8, day: u8) -> bool {
    if !(1990..=2099).contains(&year) || !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let mut dim = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month as usize];
    if month == 2 && (year % 400 == 0 || (year % 4 == 0 && year % 100 != 0)) {
        dim = 29;
    }
    day <= dim
}

fn format_stamp(sep: char, has_second: bool, year: u16, month: u8, day: u8, hour: u8, minute: u8, second: Option<u8>) -> String {
    if has_second {
        format!(
            "{year:04}{sep}{month:02}{sep}{day:02} {hour:02}:{minute:02}:{:02}",
            second.unwrap_or(0)
        )
    } else {
        format!("{year:04}{sep}{month:02}{sep}{day:02} {hour:02}:{minute:02}")
    }
}

fn ink_color(image: &RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32) -> [u8; 3] {
    let mut samples = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(px) = pixel(image, x, y) {
                samples.push((luma_of(px), [px[0], px[1], px[2]]));
            }
        }
    }
    if samples.is_empty() {
        return [255, 255, 255];
    }
    samples.sort_by_key(|s| s.0);
    let start = samples.len() * 3 / 4;
    let slice = &samples[start..];
    let n = slice.len().max(1) as u32;
    let mut acc = [0u32; 3];
    for (_, rgb) in slice {
        acc[0] += rgb[0] as u32;
        acc[1] += rgb[1] as u32;
        acc[2] += rgb[2] as u32;
    }
    [ (acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8 ]
}

fn background(image: &RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32) -> ([u8; 3], i32) {
    let mut samples = Vec::new();
    for y in y0..=y1 {
        for x in (x0 - 14)..(x0 - 2) {
            if let Some(px) = pixel(image, x, y) {
                samples.push(px);
            }
        }
        for x in (x1 + 2)..=(x1 + 14) {
            if let Some(px) = pixel(image, x, y) {
                samples.push(px);
            }
        }
    }
    if samples.len() < 20 {
        for y in (y1 + 3)..=(y1 + 10) {
            for x in x0..=x1 {
                if let Some(px) = pixel(image, x, y) {
                    samples.push(px);
                }
            }
        }
    }
    if samples.is_empty() {
        return ([0, 0, 0], 28);
    }
    let mut lumas: Vec<i32> = samples.iter().map(|p| luma_of(*p)).collect();
    lumas.sort_unstable();
    let mid = lumas[lumas.len() / 2];
    let mut chans = [Vec::new(), Vec::new(), Vec::new()];
    for px in &samples {
        for c in 0..3 {
            chans[c].push(px[c]);
        }
    }
    let mut color = [0u8; 3];
    for c in 0..3 {
        chans[c].sort_unstable();
        color[c] = chans[c][chans[c].len() / 2];
    }
    (color, mid + 28)
}

fn erase_and_inpaint(image: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, bg: [u8; 3], cutoff: i32) {
    let x0 = (x0 - 3).max(0);
    let y0 = (y0 - 3).max(0);
    let x1 = (x1 + 3).min(image.width() as i32 - 1);
    let y1 = (y1 + 3).min(image.height() as i32 - 1);
    if x1 < x0 || y1 < y0 {
        return;
    }
    let bw = (x1 - x0 + 1) as usize;
    let bh = (y1 - y0 + 1) as usize;
    let mut rgb = vec![[0u8; 3]; bw * bh];
    let mut hole = vec![false; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            let px = image.get_pixel((x0 as usize + x) as u32, (y0 as usize + y) as u32).0;
            let i = y * bw + x;
            rgb[i] = [px[0], px[1], px[2]];
            if luma_of(px) > cutoff {
                hole[i] = true;
            }
        }
    }
    // The gray fringe of a white glyph sits only a little above the photo,
    // so a luma cutoff leaves a ghost. Spread the bright core by two pixels.
    let core = hole.clone();
    for y in 0..bh {
        for x in 0..bw {
            if !core[y * bw + x] {
                continue;
            }
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let xx = x as i32 + dx;
                    let yy = y as i32 + dy;
                    if xx >= 0 && yy >= 0 && xx < bw as i32 && yy < bh as i32 {
                        hole[yy as usize * bw + xx as usize] = true;
                    }
                }
            }
        }
    }
    for i in 0..hole.len() {
        if hole[i] {
            rgb[i] = bg;
        }
    }
    for _ in 0..18 {
        let prev = rgb.clone();
        for y in 0..bh {
            for x in 0..bw {
                let i = y * bw + x;
                if !hole[i] {
                    continue;
                }
                let mut acc = [0u32; 3];
                let mut n = 0u32;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let xx = x as i32 + dx;
                        let yy = y as i32 + dy;
                        if xx < 0 || yy < 0 || xx >= bw as i32 || yy >= bh as i32 {
                            continue;
                        }
                        let p = prev[yy as usize * bw + xx as usize];
                        acc[0] += p[0] as u32;
                        acc[1] += p[1] as u32;
                        acc[2] += p[2] as u32;
                        n += 1;
                    }
                }
                if n > 0 {
                    rgb[i] = [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8];
                }
            }
        }
    }
    for y in 0..bh {
        for x in 0..bw {
            let i = y * bw + x;
            if !hole[i] {
                continue;
            }
            let px = image.get_pixel_mut((x0 as usize + x) as u32, (y0 as usize + y) as u32);
            px.0[0] = rgb[i][0];
            px.0[1] = rgb[i][1];
            px.0[2] = rgb[i][2];
        }
    }
}

fn px_for_height(target: u32) -> f32 {
    let Some(font) = font() else {
        return target as f32;
    };
    let mut best = target as f32;
    let mut best_diff = i32::MAX;
    let low = (target as i32 - 10).max(8);
    let high = target as i32 + 18;
    for px in low..=high {
        let (metrics, _) = font.rasterize('0', px as f32);
        let diff = (metrics.height as i32 - target as i32).abs();
        if diff < best_diff {
            best_diff = diff;
            best = px as f32;
        }
    }
    best
}

fn draw_text(image: &mut RgbaImage, text: &str, cx: i32, cy: i32, px: f32, color: [u8; 3]) {
    let Some(font) = font() else {
        return;
    };
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    let mut ink_top = i32::MAX;
    let mut ink_bot = i32::MIN;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, px);
        let top = -(metrics.ymin + metrics.height as i32);
        if metrics.width > 0 {
            ink_top = ink_top.min(top);
            ink_bot = ink_bot.max(top + metrics.height as i32);
        }
        glyphs.push((pen, metrics, bitmap, top));
        pen += metrics.advance_width;
    }
    if glyphs.is_empty() || pen <= 0.0 || ink_top == i32::MAX {
        return;
    }
    let ink_h = (ink_bot - ink_top).max(1);
    let baseline = cy - (ink_top + ink_h / 2);
    let origin = cx as f32 - pen / 2.0;
    let w = image.width() as i32;
    let h = image.height() as i32;
    for (pen, metrics, bitmap, top) in glyphs {
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let x = (origin + pen).round() as i32 + metrics.xmin;
        let y = baseline + top;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let cov = bitmap[row * metrics.width + col];
                if cov == 0 {
                    continue;
                }
                let xx = x + col as i32;
                let yy = y + row as i32;
                if xx < 0 || yy < 0 || xx >= w || yy >= h {
                    continue;
                }
                let dst = image.get_pixel_mut(xx as u32, yy as u32);
                let c = cov as f32 / 255.0;
                for ch in 0..3 {
                    let mixed = color[ch] as f32 * c + dst.0[ch] as f32 * (1.0 - c);
                    dst.0[ch] = mixed.round() as u8;
                }
            }
        }
    }
}

fn luma_at(image: &RgbaImage, x: i32, y: i32) -> i32 {
    luma_of(image.get_pixel(x as u32, y as u32).0)
}

fn luma_of(px: [u8; 4]) -> i32 {
    (px[0] as i32 * 30 + px[1] as i32 * 59 + px[2] as i32 * 11) / 100
}

fn pixel(image: &RgbaImage, x: i32, y: i32) -> Option<[u8; 4]> {
    if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
        None
    } else {
        Some(image.get_pixel(x as u32, y as u32).0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn painted(text: &str) -> RgbaImage {
        let mut image = RgbaImage::new(720, 220);
        for y in 0..220 {
            for x in 0..720 {
                let v = (18 + ((x * 3 + y) % 17)) as u8;
                image.put_pixel(x, y, Rgba([v, v, v.saturating_add(6), 255]));
            }
        }
        draw_text(&mut image, text, 360, 110, 32.0, [255, 255, 255]);
        image
    }

    #[test]
    fn redraws_a_date_and_keeps_the_format() {
        let mut image = painted("2026/11/15 16:08");
        let before = image.clone();
        let found = detect_camera_time(&image).expect("date");
        assert_eq!(found.text, "2026/11/15 16:08");
        assert_eq!(found.second, None);
        let change = edit_camera_time(
            &mut image,
            CameraTimeValue {
                year: 2026,
                month: 12,
                day: 1,
                hour: 9,
                minute: 30,
                second: Some(45),
            },
        )
        .unwrap();
        assert_eq!(change.original, "2026/11/15 16:08");
        assert_eq!(change.updated, "2026/12/01 09:30");
        let again = detect_camera_time(&image).expect("rewritten");
        assert_eq!(again.text, "2026/12/01 09:30");
        assert_eq!(again.second, None);
        assert_eq!(image.get_pixel(4, 4).0, before.get_pixel(4, 4).0);
        let mut bright = 0u32;
        for y in 70..150 {
            for x in 200..520 {
                if image.get_pixel(x, y).0[0] > 180 {
                    bright += 1;
                }
            }
        }
        assert!(bright > 40, "rewritten ink {bright}");
    }

    #[test]
    fn keeps_seconds_when_the_stamp_has_them() {
        let mut image = painted("2026-11-15 16:08:07");
        let found = detect_camera_time(&image).expect("date");
        assert_eq!(found.separator, '-');
        assert_eq!(found.second, Some(7));
        edit_camera_time(
            &mut image,
            CameraTimeValue {
                year: 2024,
                month: 2,
                day: 29,
                hour: 0,
                minute: 1,
                second: Some(2),
            },
        )
        .unwrap();
        let again = detect_camera_time(&image).expect("rewritten");
        assert_eq!(again.text, "2024-02-29 00:01:02");
    }

    #[test]
    fn missing_stamp_does_not_change_pixels() {
        let mut image = painted("");
        let before = image.clone();
        let err = edit_camera_time(
            &mut image,
            CameraTimeValue { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: None },
        )
        .unwrap_err();
        assert!(matches!(err, CameraTimeError::NotFound));
        assert_eq!(image.as_raw(), before.as_raw());
    }

    #[test]
    fn invalid_calendar_date_is_rejected() {
        let mut image = painted("2026/11/15 16:08");
        let before = image.clone();
        let err = edit_camera_time(
            &mut image,
            CameraTimeValue { year: 2023, month: 2, day: 29, hour: 12, minute: 0, second: None },
        )
        .unwrap_err();
        assert!(matches!(err, CameraTimeError::Invalid));
        assert_eq!(image.as_raw(), before.as_raw());
    }

    #[test]
    fn light_wall_stamp_with_dots_is_found() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/20261105-0912.jpg");
        let input = crate::image_io::decode_path(&path).expect("panel photo");
        let found = detect_camera_time(&input.rgba).expect("date on the light wall");
        assert_eq!(found.text, "2026.11.05 09:12");
        assert_eq!(found.separator, '.');
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
        let change = edit_camera_time(
            &mut image,
            CameraTimeValue { year: 2026, month: 12, day: 1, hour: 9, minute: 30, second: Some(45) },
        )
        .unwrap();
        assert_eq!(change.original, "2026.11.05 09:12");
        assert_eq!(change.updated, "2026.12.01 09:30");
        let again = detect_camera_time(&image).expect("rewritten dotted date");
        assert_eq!(again.text, "2026.12.01 09:30");
        assert_eq!(again.separator, '.');
        let mut stray = 0u32;
        for y in 0..image.height() {
            for x in 0..2200 {
                if image.get_pixel(x, y).0 != input.rgba.get_pixel(x, y).0 {
                    stray += 1;
                }
            }
        }
        assert_eq!(stray, 0, "pixels changed outside the date on the right");
    }

    #[test]
    fn sample_photo_date_can_be_rewritten() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/20261119-092612.jpg");
        let input = crate::image_io::decode_path(&path).expect("sample");
        let found = detect_camera_time(&input.rgba).expect("huawei date");
        assert_eq!(found.text, "2026/11/15 16:08");
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
        let change = edit_camera_time(
            &mut image,
            CameraTimeValue { year: 2026, month: 12, day: 1, hour: 9, minute: 30, second: Some(45) },
        )
        .unwrap();
        assert_eq!(change.updated, "2026/12/01 09:30");
        let again = detect_camera_time(&image).expect("rewritten sample");
        assert_eq!(again.text, "2026/12/01 09:30");
        let mut stray = 0u32;
        for y in 0..image.height() {
            for x in 0..image.width() {
                if image.get_pixel(x, y).0 != input.rgba.get_pixel(x, y).0 {
                    let inside = (980..1400).contains(&x) && (1540..1640).contains(&y);
                    if !inside {
                        stray += 1;
                    }
                }
            }
        }
        assert_eq!(stray, 0, "pixels changed outside the date line");
    }

    #[test]
    fn date_edit_leaves_the_doubao_result_and_the_model_line() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let templates = crate::template::load_template_dir(&root.join("resources/doubao")).expect("templates");
        let input = crate::image_io::decode_path(&root.join("docs/20261119-092612.jpg")).expect("sample");
        let off = crate::restore::process_best(&input, &templates, &crate::restore::ProcessOptions::default()).unwrap();
        let on = crate::restore::process_best(
            &input,
            &templates,
            &crate::restore::ProcessOptions {
                camera_time: Some(CameraTimeValue {
                    year: 2026,
                    month: 12,
                    day: 1,
                    hour: 9,
                    minute: 30,
                    second: None,
                }),
                ..crate::restore::ProcessOptions::default()
            },
        )
        .unwrap();
        assert!(off.report.camera_time.is_none());
        assert!(!off.report.flags.iter().any(|flag| flag.starts_with("camera_time")));
        assert_eq!(off.report.counts.outside_diff_pixels, 0);
        assert_eq!(on.report.counts.outside_diff_pixels, 0);
        let change = on.report.camera_time.expect("edited");
        assert_eq!(change.original, "2026/11/15 16:08");
        assert_eq!(change.updated, "2026/12/01 09:30");
        assert!(on.report.flags.iter().any(|flag| flag == "camera_time_edited"));
        let off_img = off.image.expect("restored");
        let on_img = on.image.expect("restored with date");
        assert_eq!(on_img.get_pixel(900, 1510).0, input.rgba.get_pixel(900, 1510).0);
        let [x, y, w, h] = on.report.bbox.expect("doubao box");
        for py in y..y + h {
            for px in x..x + w {
                assert_eq!(
                    on_img.get_pixel(px as u32, py as u32).0,
                    off_img.get_pixel(px as u32, py as u32).0,
                    "doubao pixel {px},{py}"
                );
            }
        }
    }
}
