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
    /// Glyph pixels in `x0..=x1`, `y0..=y1`, row-major. The picture between them stays.
    ink: Vec<u8>,
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
    valid_clock(value.hour, value.minute, value.second)
        && valid_date(value.year, value.month, value.day)
}

pub fn edit_camera_time(
    image: &mut RgbaImage,
    value: CameraTimeValue,
) -> Result<CameraTimeChange, CameraTimeError> {
    if !is_valid_camera_time(value) {
        return Err(CameraTimeError::Invalid);
    }
    let stamp = find_stamp(image).ok_or(CameraTimeError::NotFound)?;
    let second = if stamp.has_second {
        value
            .second
            .or_else(|| parse_stamp(&stamp.text).and_then(|p| p.5))
    } else {
        None
    };
    let updated = format_stamp(
        stamp.separator,
        stamp.has_second,
        value.year,
        value.month,
        value.day,
        value.hour,
        value.minute,
        second,
    );
    let color = ink_color(image, stamp.x0, stamp.y0, stamp.x1, stamp.y1);
    erase_stamp(image, &stamp);
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
    if let Some(stamp) = find_at(image, 150, 28, top) {
        return Some(stamp);
    }
    // A stamp on white plastic is only a few levels brighter than the duct,
    // and a dark seam beside a glyph makes the darkest nearby pixel look
    // like the background. Compare with the local median in the bottom-right
    // corner, which is where these camera stamps sit.
    find_soft(image)
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
    stamp_from_mask(&mask, w, h, false)
}

fn find_soft(image: &RgbaImage) -> Option<Stamp> {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if w < 16 || h < 16 {
        return None;
    }
    let y0 = (h - (h / 5).max(80)).max(0);
    let x0 = w * 45 / 100;
    let mut mask = vec![0u8; (w * h) as usize];
    let radius = 18i32;
    let step = 6i32;
    let mut samples = [0i32; 49];
    for y in y0..h {
        for x in x0..w {
            let lum = luma_at(image, x, y);
            if lum < 150 {
                continue;
            }
            let mut n = 0usize;
            let mut dy = -radius;
            while dy <= radius {
                let yy = y + dy;
                let mut dx = -radius;
                while dx <= radius {
                    let xx = x + dx;
                    if xx >= 0 && yy >= 0 && xx < w && yy < h {
                        samples[n] = luma_at(image, xx, yy);
                        n += 1;
                    }
                    dx += step;
                }
                dy += step;
            }
            if n < 20 {
                continue;
            }
            samples[..n].sort_unstable();
            if lum - samples[n / 2] >= 5 {
                mask[(y * w + x) as usize] = 1;
            }
        }
    }
    // Single-pixel JPEG specks stick to a glyph and turn a 9 into a 7.
    // A solid stroke still has several neighbors.
    denoise_mask(&mut mask, w, h, x0, y0);
    // A duct ridge is a hairline that joins a digit to a tall edge. The
    // whole component is then taller than a glyph and gets discarded.
    // Peel those hairlines off, and leave a thin "1" alone.
    detach_ridges(&mut mask, w, h);
    stamp_from_mask(&mask, w, h, true)
}

fn denoise_mask(mask: &mut [u8], w: i32, h: i32, x0: i32, y0: i32) {
    let mut keep = vec![0u8; mask.len()];
    for y in y0..h {
        for x in x0..w {
            let i = (y * w + x) as usize;
            if mask[i] == 0 {
                continue;
            }
            let mut n = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let xx = x + dx;
                    let yy = y + dy;
                    if xx >= 0 && yy >= 0 && xx < w && yy < h && mask[(yy * w + xx) as usize] != 0 {
                        n += 1;
                    }
                }
            }
            let up = y > 0 && mask[((y - 1) * w + x) as usize] != 0;
            let down = y + 1 < h && mask[((y + 1) * w + x) as usize] != 0;
            // A one-pixel stem still has a neighbor above and below. An isolated speck does not.
            if n >= 4 || (n >= 3 && up && down) {
                keep[i] = 1;
            }
        }
    }
    for y in y0..h {
        for x in x0..w {
            let i = (y * w + x) as usize;
            mask[i] = keep[i];
        }
    }
}

fn detach_ridges(mask: &mut [u8], w: i32, h: i32) {
    let mut hrun = vec![0i32; mask.len()];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if mask[(y * w + x) as usize] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            x += 1;
            while x < w && mask[(y * w + x) as usize] != 0 {
                x += 1;
            }
            for xx in start..x {
                hrun[(y * w + xx) as usize] = x - start;
            }
        }
    }
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    let mut pts = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let start = (y * w + x) as usize;
            if mask[start] == 0 || seen[start] {
                continue;
            }
            pts.clear();
            stack.clear();
            stack.push((x, y));
            seen[start] = true;
            let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
            while let Some((cx, cy)) = stack.pop() {
                pts.push((cx, cy));
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
            if y1 - y0 + 1 <= 64 {
                continue;
            }
            for &(px, py) in &pts {
                let i = (py * w + px) as usize;
                if hrun[i] <= 2 {
                    mask[i] = 0;
                }
            }
        }
    }
}

fn stamp_from_mask(mask: &[u8], w: i32, h: i32, trim: bool) -> Option<Stamp> {
    let comps = components(mask, w, h);
    let mut lines = group_lines(&comps);
    lines.sort_by_key(|line| std::cmp::Reverse(line_bounds(&comps, line).3));
    for line in lines {
        // The model name and the date share a baseline. A wide gap keeps the
        // date from being swallowed by the longer model line.
        for part in split_wide_gaps(&comps, &line) {
            if let Some(stamp) = read_line(mask, w, &comps, &part, trim) {
                return Some(stamp);
            }
        }
    }
    None
}

fn darkest_ring(image: &RgbaImage, x: i32, y: i32, w: i32, h: i32) -> i32 {
    const RING: [(i32, i32); 8] = [
        (8, 0),
        (-8, 0),
        (0, 8),
        (0, -8),
        (6, 6),
        (6, -6),
        (-6, 6),
        (-6, -6),
    ];
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
            comps.push(Comp {
                x0,
                y0,
                x1,
                y1,
                area,
            });
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
    let mut heights: Vec<i32> = idxs
        .iter()
        .map(|&i| comps[i].y1 - comps[i].y0 + 1)
        .collect();
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

fn merge_stacked(comps: Vec<Comp>, min_narrow: i32) -> Vec<Comp> {
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
            // A broken digit stacks two pieces. On the soft pass a narrow
            // fragment may join too, but a short separator dot must not.
            let shorter = (prev.y1 - prev.y0 + 1).min(comp.y1 - comp.y0 + 1);
            let taller = (prev.y1 - prev.y0 + 1).max(comp.y1 - comp.y0 + 1);
            let alike = min_narrow >= 10 || shorter * 3 >= taller;
            if overlap > 0 && overlap * 2 >= narrow && narrow >= min_narrow && vgap <= 4 && alike {
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

fn on_the_digit_row(comps: Vec<Comp>, median_h: i32) -> Vec<Comp> {
    if comps.is_empty() {
        return comps;
    }
    let mut centers: Vec<i32> = comps.iter().map(|comp| (comp.y0 + comp.y1) / 2).collect();
    centers.sort_unstable();
    let mid = centers[centers.len() / 2];
    comps
        .into_iter()
        .filter(|comp| {
            let cy = (comp.y0 + comp.y1) / 2;
            (cy - mid).abs() * 5 <= median_h * 3
        })
        .collect()
}

fn join_touching(comps: Vec<Comp>, median_h: i32) -> Vec<Comp> {
    let mut out: Vec<Comp> = Vec::new();
    for comp in comps {
        if let Some(prev) = out.last_mut() {
            let gap = comp.x0 - prev.x1 - 1;
            let union_w = comp.x1.max(prev.x1) - comp.x0.min(prev.x0) + 1;
            let overlap_y = prev.y1.min(comp.y1) - prev.y0.max(comp.y0) + 1;
            // Fragments of one thin digit touch. Two neighboring digits together
            // are wider than the line is tall.
            if gap <= 1 && overlap_y > 0 && union_w <= median_h {
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

fn x_groups(comps: &[Comp], median_h: i32) -> Vec<Vec<Comp>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for comp in comps {
        if let Some(prev) = current.last() {
            let prev: &Comp = prev;
            if comp.x0 - prev.x1 - 1 > median_h * 2 {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(*comp);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn read_line(mask: &[u8], w: i32, comps: &[Comp], line: &[usize], trim: bool) -> Option<Stamp> {
    // Dots and colon marks sit on the same line and are dropped after the
    // height filter, so the raw count is wider than the digit count. The soft
    // pass also keeps ridge fragments, so its raw count is wider still.
    let limit = if trim { 120 } else { 32 };
    if !(8..=limit).contains(&line.len()) {
        return None;
    }
    let mut parts: Vec<Comp> = line.iter().map(|&i| comps[i]).collect();
    parts.sort_by_key(|comp| (comp.x0, comp.y0));
    let mut parts = merge_stacked(parts, if trim { 4 } else { 10 });
    if trim {
        // A cable-slot edge is a short bar much wider than a digit.
        parts.retain(|comp| {
            let bw = comp.x1 - comp.x0 + 1;
            let bh = comp.y1 - comp.y0 + 1;
            !(bw > bh * 2 && bw > 36)
        });
    }
    if parts.is_empty() {
        return None;
    }
    let mut heights: Vec<i32> = parts.iter().map(|comp| comp.y1 - comp.y0 + 1).collect();
    heights.sort_unstable();
    let median_h = if trim {
        let upper = &heights[heights.len() / 2..];
        upper[upper.len() / 2]
    } else {
        heights[heights.len() / 2]
    };
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
    let groups = if trim {
        let kept = join_touching(on_the_digit_row(kept, median_h), median_h);
        x_groups(&kept, median_h)
    } else if (10..=16).contains(&kept.len()) {
        vec![kept]
    } else {
        return None;
    };
    for kept in groups {
        if let Some(stamp) = read_glyphs(mask, w, kept, median_h, trim) {
            return Some(stamp);
        }
    }
    None
}

fn read_glyphs(mask: &[u8], w: i32, kept: Vec<Comp>, median_h: i32, trim: bool) -> Option<Stamp> {
    if !(10..=16).contains(&kept.len()) {
        return None;
    }
    // A one-pixel JPEG fringe on a soft mask turns a 9 into a 7. The stricter
    // passes already match the glyph, and trimming them drops the score.
    let kept: Vec<Comp> = if trim {
        let mut centers: Vec<i32> = kept.iter().map(|comp| (comp.y0 + comp.y1) / 2).collect();
        centers.sort_unstable();
        let mid = centers[centers.len() / 2];
        kept.into_iter()
            .map(|comp| {
                let mut comp = trim_fringe(mask, w, comp);
                // A ridge sticks out above and below the digit and stretches the template.
                let y0 = comp.y0.max(mid - median_h / 2);
                let y1 = comp.y1.min(mid + median_h / 2);
                if y1 - y0 >= 6 {
                    comp.y0 = y0;
                    comp.y1 = y1;
                }
                comp
            })
            .collect()
    } else {
        kept
    };
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
    // A "1" is much narrower than the other digits, so the empty gap beside it
    // looks like a word space. Measure the gap as if both neighbors had a
    // normal digit width.
    let typical = median_h * 6 / 10;
    let space_width = |n: usize, width: i32| {
        let prev = kept[n].x1 - kept[n].x0 + 1;
        let next = kept[n + 1].x1 - kept[n + 1].x0 + 1;
        let pad = (typical - prev).max(0) / 2 + (typical - next).max(0) / 2;
        width - pad
    };
    let mut empty_widths: Vec<i32> = gaps
        .iter()
        .enumerate()
        .filter_map(|(n, gap)| {
            gap.as_ref()
                .filter(|gap| gap.ink < 3)
                .map(|gap| space_width(n, gap.width))
        })
        .collect();
    empty_widths.sort_unstable();
    // Narrow digits such as "1" leave a gap that is almost as wide as a word
    // space. When one empty gap is clearly wider than the others, only that
    // one is the space.
    let space_at = if empty_widths.len() >= 2 {
        let largest = empty_widths[empty_widths.len() - 1];
        let next = empty_widths[empty_widths.len() - 2];
        if largest * 10 >= next * 14 {
            next + 1
        } else {
            -1
        }
    } else {
        -1
    };
    let mut text = String::new();
    // Dots, hyphens and colons are gaps, not digit components. Their pixels
    // still have to be erased or the old mark stays beside the new time.
    let mut separators = Vec::new();
    for (n, comp) in kept.iter().enumerate() {
        if n > 0 {
            if let Some(gap) = &gaps[n - 1] {
                match gap_char(
                    gap,
                    y0,
                    y1,
                    median_h,
                    space_at,
                    space_width(n - 1, gap.width),
                ) {
                    Some('\0') => {}
                    Some(ch) => {
                        if matches!(ch, '-' | '.' | ':') && gap.ink >= 3 {
                            separators.push((gap.ix0, gap.iy0, gap.ix1, gap.iy1));
                        }
                        text.push(ch);
                    }
                    None => return None,
                }
            }
        }
        let ch = match classify(mask, w, *comp, &glyphs) {
            Some(ch) => ch,
            // A slot edge stuck to a faint "1" never matches the font. The flag
            // is wider than the stem, which a bare slot edge is not.
            None if trim && narrow_one(mask, w, *comp) => '1',
            None if trim => match thicken(mask, w, *comp, &glyphs) {
                Some(ch) => ch,
                None => return None,
            },
            None => return None,
        };
        text.push(ch);
    }
    let parsed = match parse_stamp(&text) {
        Some(parsed) => parsed,
        None => return None,
    };
    Some(Stamp {
        x0,
        y0,
        x1,
        y1,
        height: median_h,
        separator: parsed.6,
        has_second: parsed.5.is_some(),
        text,
        ink: glyph_ink(mask, w, &kept, &separators, x0, y0, x1, y1),
    })
}

fn glyph_ink(
    mask: &[u8],
    w: i32,
    kept: &[Comp],
    separators: &[(i32, i32, i32, i32)],
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Vec<u8> {
    let mw = (x1 - x0 + 1).max(0) as usize;
    let mh = (y1 - y0 + 1).max(0) as usize;
    let mut ink = vec![0u8; mw.saturating_mul(mh)];
    if mw == 0 || mh == 0 {
        return ink;
    }
    for comp in kept {
        for y in comp.y0..=comp.y1 {
            for x in comp.x0..=comp.x1 {
                if y < 0 || x < 0 || x >= w {
                    continue;
                }
                let src = (y * w + x) as usize;
                if src >= mask.len() || mask[src] == 0 {
                    continue;
                }
                let ix = (x - x0) as usize;
                let iy = (y - y0) as usize;
                if ix < mw && iy < mh {
                    ink[iy * mw + ix] = 1;
                }
            }
        }
    }
    for &(sx0, sy0, sx1, sy1) in separators {
        for y in sy0..=sy1 {
            for x in sx0..=sx1 {
                if y < y0 || x < x0 || y > y1 || x > x1 || x >= w || x < 0 || y < 0 {
                    continue;
                }
                let src = (y * w + x) as usize;
                if src >= mask.len() || mask[src] == 0 {
                    continue;
                }
                ink[(y - y0) as usize * mw + (x - x0) as usize] = 1;
            }
        }
    }
    ink
}

struct GapInk {
    width: i32,
    ink: u32,
    runs: u32,
    /// Tallest vertical run. A colon dot is short; a broken slot edge is not.
    max_run: i32,
    /// Longest empty stretch after a real run, before the next ink.
    hole: i32,
    gh: i32,
    gw: i32,
    mid: i32,
    ix0: i32,
    iy0: i32,
    ix1: i32,
    iy1: i32,
    /// Ink in the top and bottom thirds with an empty middle: the two dots of a colon.
    colon_like: bool,
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
    // A single bright row is JPEG noise. A colon dot and a decimal dot are taller.
    let mut runs = 0u32;
    let mut max_run = 0i32;
    let mut hole = 0i32;
    let mut gap_len = 0i32;
    let mut seen_run = false;
    let mut prev = false;
    let mut run_len = 0i32;
    for on in &rows {
        if *on {
            if seen_run {
                hole = hole.max(gap_len);
            }
            gap_len = 0;
            run_len += 1;
            prev = true;
        } else {
            if prev && run_len > 1 {
                runs += 1;
                max_run = max_run.max(run_len);
                seen_run = true;
            }
            if seen_run {
                gap_len += 1;
            }
            prev = false;
            run_len = 0;
        }
    }
    if prev && run_len > 1 {
        runs += 1;
        max_run = max_run.max(run_len);
    }
    let third = rows.len() / 3;
    let colon_like = third > 0
        && rows[..third].iter().any(|on| *on)
        && rows[rows.len() - third..].iter().any(|on| *on)
        && rows[third..rows.len() - third].iter().all(|on| !*on);
    GapInk {
        width: x1 - x0 + 1,
        ink,
        runs,
        max_run,
        hole,
        gh: if ink == 0 { 0 } else { iy1 - iy0 + 1 },
        gw: if ink == 0 { 0 } else { ix1 - ix0 + 1 },
        mid: if ink == 0 { 0 } else { (iy0 + iy1) / 2 },
        ix0,
        iy0,
        ix1,
        iy1,
        colon_like,
    }
}

fn gap_char(
    gap: &GapInk,
    y0: i32,
    y1: i32,
    line_h: i32,
    space_at: i32,
    space_width: i32,
) -> Option<char> {
    let wide = if space_at >= 0 {
        space_width >= space_at
    } else {
        space_width * 10 > line_h * 3
    };
    if gap.ink < 3 {
        return if wide { Some(' ') } else { Some('\0') };
    }
    // A crack between digits, or a seam only a few pixels wide, is not a colon.
    // A broken slot edge is two long runs; a colon is two short dots.
    let short_dots = gap.runs == 2 && gap.max_run * 3 <= line_h;
    // The seam through a cabinet colon stretches one dot, but the two marks
    // are still separated. A broken slot edge has only a few empty rows.
    let split_dots = gap.runs == 2 && gap.hole >= 8 && gap.max_run * 2 <= line_h;
    if gap.width >= 8
        && (short_dots || split_dots || (gap.colon_like && gap.gw * 2 <= line_h && gap.ink < 250))
    {
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
    // A slot edge in the gap is one tall, thin mark. It is not a separator.
    if gap.max_run * 3 >= line_h && gap.gw * 3 <= line_h {
        return Some('\0');
    }
    if wide {
        None
    } else {
        Some('\0')
    }
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

/// A faint "1" with a slot edge stuck to its right. The font score stays low
/// because the stem is only a couple of levels above a white duct, so the shape
/// is checked directly: a flag wider than the stem, and not a bare vertical bar.
fn narrow_one(mask: &[u8], w: i32, comp: Comp) -> bool {
    let bw = comp.x1 - comp.x0 + 1;
    let bh = comp.y1 - comp.y0 + 1;
    if bw < 3 || bh < 12 {
        return false;
    }
    let mut cols = vec![0i32; bw as usize];
    for y in comp.y0..=comp.y1 {
        for x in comp.x0..=comp.x1 {
            if mask[(y * w + x) as usize] != 0 {
                cols[(x - comp.x0) as usize] += 1;
            }
        }
    }
    let mut right = bw as usize;
    let low = 4.max(bh / 8);
    let tall = bh * 4 / 5;
    if let Some(bar_at) = cols
        .iter()
        .enumerate()
        .rev()
        .find(|(_, c)| **c >= tall)
        .map(|(i, _)| i)
    {
        if bar_at + 5 >= cols.len() {
            let mut bar_left = bar_at;
            while bar_left > 0 && cols[bar_left - 1] > low && bar_at + 1 - (bar_left - 1) <= 4 {
                bar_left -= 1;
            }
            if bar_at + 1 - bar_left <= 4 && bar_left >= 4 {
                let mut valley_left = bar_left;
                let mut valley = 0i32;
                while valley_left > 0 && cols[valley_left - 1] <= low {
                    valley_left -= 1;
                    valley += 1;
                }
                if valley >= 4 && cols[..valley_left].iter().copied().max().unwrap_or(0) >= bh / 2 {
                    right = valley_left;
                }
            }
        }
    }
    let kept = right as i32;
    if kept < 3 || kept * 10 > bh * 5 {
        return false;
    }
    let top_end = comp.y0 + bh * 35 / 100;
    let stem_from = comp.y0 + bh / 2;
    let mut top_width = 0i32;
    let mut stem_widths = Vec::new();
    let mut rows_on = 0i32;
    let mut middle_on = false;
    let mid0 = comp.y0 + bh / 3;
    let mid1 = comp.y0 + bh * 2 / 3;
    for y in comp.y0..=comp.y1 {
        let mut n = 0i32;
        for x in comp.x0..comp.x0 + kept {
            if mask[(y * w + x) as usize] != 0 {
                n += 1;
            }
        }
        if n > 0 {
            rows_on += 1;
        }
        if y <= top_end {
            top_width = top_width.max(n);
        }
        if y >= stem_from {
            stem_widths.push(n);
        }
        if y >= mid0 && y <= mid1 && n > 0 {
            middle_on = true;
        }
    }
    if !middle_on || rows_on * 10 < bh * 6 || stem_widths.is_empty() {
        return false;
    }
    stem_widths.sort_unstable();
    let stem = stem_widths[stem_widths.len() / 2];
    // A slot edge is the same width from top to bottom. A "1" has a flag.
    stem >= 1 && top_width >= stem + 2
}

fn trim_fringe(mask: &[u8], w: i32, mut comp: Comp) -> Comp {
    let row_ink = |y: i32, comp: Comp| -> i32 {
        (comp.x0..=comp.x1)
            .filter(|&x| mask[(y * w + x) as usize] != 0)
            .count() as i32
    };
    let col_ink = |x: i32, comp: Comp| -> i32 {
        (comp.y0..=comp.y1)
            .filter(|&y| mask[(y * w + x) as usize] != 0)
            .count() as i32
    };
    while comp.y0 < comp.y1 && row_ink(comp.y0, comp) < 3 {
        comp.y0 += 1;
    }
    while comp.y1 > comp.y0 && row_ink(comp.y1, comp) < 3 {
        comp.y1 -= 1;
    }
    while comp.x0 < comp.x1 && col_ink(comp.x0, comp) < 3 {
        comp.x0 += 1;
    }
    while comp.x1 > comp.x0 && col_ink(comp.x1, comp) < 3 {
        comp.x1 -= 1;
    }
    comp
}

fn classify(mask: &[u8], w: i32, comp: Comp, glyphs: &[GlyphBmp]) -> Option<char> {
    let (ch, score) = classify_scored(mask, w, comp, glyphs);
    if score >= 0.38 {
        Some(ch)
    } else {
        None
    }
}

/// A thin stroke on a white surface is the same digit, one pixel short of the
/// bold template. Thicken it only when the thin shape already prefers that digit.
fn thicken(mask: &[u8], w: i32, comp: Comp, glyphs: &[GlyphBmp]) -> Option<char> {
    let (ch, score) = classify_scored(mask, w, comp, glyphs);
    if score < 0.25 || !ch.is_ascii_digit() {
        return None;
    }
    let mut thick = mask.to_vec();
    for y in comp.y0..=comp.y1 {
        for x in comp.x0..=comp.x1 {
            let i = (y * w + x) as usize;
            if mask[i] != 0 {
                continue;
            }
            let near = [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().any(|&(dy, dx)| {
                let yy = y + dy;
                let xx = x + dx;
                (comp.y0..=comp.y1).contains(&yy)
                    && (comp.x0..=comp.x1).contains(&xx)
                    && mask[(yy * w + xx) as usize] != 0
            });
            if near {
                thick[i] = 1;
            }
        }
    }
    let (again, score) = classify_scored(&thick, w, comp, glyphs);
    if again == ch && score >= 0.38 {
        Some(ch)
    } else {
        None
    }
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
    let scaled_w = ((glyph.width * dh) as f32 / glyph.height as f32)
        .round()
        .max(1.0) as usize;
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
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
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

fn format_stamp(
    sep: char,
    has_second: bool,
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: Option<u8>,
) -> String {
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
    let mut glyphs = Vec::new();
    let mut samples = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let Some(px) = pixel(image, x, y) else {
                continue;
            };
            let lum = luma_of(px);
            samples.push((lum, [px[0], px[1], px[2]]));
            if lum >= 150 && above_local(image, x, y, lum) >= 5 {
                glyphs.push((lum, [px[0], px[1], px[2]]));
            }
        }
    }
    // The brightest quarter of the whole box is the plastic on a white duct.
    // The stroke cores are the bright pixels that stand above their neighborhood.
    let from_glyphs = glyphs.len() >= 8;
    let pool = if from_glyphs {
        &mut glyphs
    } else {
        &mut samples
    };
    if pool.is_empty() {
        return [255, 255, 255];
    }
    pool.sort_by_key(|s| s.0);
    let start = if from_glyphs {
        pool.len() * 9 / 10
    } else {
        pool.len() * 3 / 4
    };
    let slice = &pool[start..];
    let n = slice.len().max(1) as u32;
    let mut acc = [0u32; 3];
    for (_, rgb) in slice {
        acc[0] += rgb[0] as u32;
        acc[1] += rgb[1] as u32;
        acc[2] += rgb[2] as u32;
    }
    [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]
}

fn above_local(image: &RgbaImage, x: i32, y: i32, lum: i32) -> i32 {
    let Some(median) = local_median_luma(image, x, y) else {
        return 0;
    };
    lum - median
}

fn local_median_luma(image: &RgbaImage, x: i32, y: i32) -> Option<i32> {
    let w = image.width() as i32;
    let h = image.height() as i32;
    let radius = 18i32;
    let step = 6i32;
    let mut samples = [0i32; 49];
    let mut n = 0usize;
    let mut dy = -radius;
    while dy <= radius {
        let yy = y + dy;
        let mut dx = -radius;
        while dx <= radius {
            let xx = x + dx;
            if xx >= 0 && yy >= 0 && xx < w && yy < h {
                samples[n] = luma_at(image, xx, yy);
                n += 1;
            }
            dx += step;
        }
        dy += step;
    }
    if n < 20 {
        return None;
    }
    samples[..n].sort_unstable();
    Some(samples[n / 2])
}

fn erase_stamp(image: &mut RgbaImage, stamp: &Stamp) {
    // The old fill painted every light pixel in the date box with one color.
    // On a wall, a duct or a gradient that becomes a rectangle. Only the
    // strokes are rebuilt, from the picture touching them.
    let pad = 16i32;
    let x0 = (stamp.x0 - pad).max(0);
    let y0 = (stamp.y0 - pad).max(0);
    let x1 = (stamp.x1 + pad).min(image.width() as i32 - 1);
    let y1 = (stamp.y1 + pad).min(image.height() as i32 - 1);
    if x1 < x0 || y1 < y0 {
        return;
    }
    let bw = (x1 - x0 + 1) as usize;
    let bh = (y1 - y0 + 1) as usize;
    let mut rgb = vec![[0u8; 3]; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            let px = image
                .get_pixel((x0 as usize + x) as u32, (y0 as usize + y) as u32)
                .0;
            rgb[y * bw + x] = [px[0], px[1], px[2]];
        }
    }
    let mut hole = vec![false; bw * bh];
    let mw = (stamp.x1 - stamp.x0 + 1).max(0) as usize;
    let mh = (stamp.y1 - stamp.y0 + 1).max(0) as usize;
    if mw == 0 || mh == 0 || stamp.ink.len() < mw * mh {
        return;
    }
    for y in stamp.y0..=stamp.y1 {
        for x in stamp.x0..=stamp.x1 {
            let ix = (x - stamp.x0) as usize;
            let iy = (y - stamp.y0) as usize;
            if stamp.ink[iy * mw + ix] == 0 {
                continue;
            }
            hole[(y - y0) as usize * bw + (x - x0) as usize] = true;
        }
    }
    cover_stroke_fringe(&mut hole, bw, bh);
    let blurred = box_blur(&rgb, bw, bh);
    expand_halo(&mut hole, &rgb, bw, bh);
    inpaint_hole(&mut rgb, &hole, bw, bh);
    restore_texture(&mut rgb, &hole, &blurred, bw, bh);
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

/// Drop a slot edge that the reader mask pulled into the date. A colon dot
/// or a decimal point is only a few pixels, so size alone must not throw it away.
fn cover_stroke_fringe(hole: &mut [bool], bw: usize, bh: usize) {
    let seeds = hole.to_vec();
    let mut seen = vec![false; hole.len()];
    let mut stack = Vec::new();
    for start in 0..hole.len() {
        if !seeds[start] || seen[start] {
            continue;
        }
        stack.clear();
        stack.push(start);
        seen[start] = true;
        let mut pts = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (start % bw, start / bw, start % bw, start / bw);
        while let Some(p) = stack.pop() {
            pts.push(p);
            let x = p % bw;
            let y = p / bw;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let xx = x as i32 + dx;
                let yy = y as i32 + dy;
                if xx < 0 || yy < 0 || xx >= bw as i32 || yy >= bh as i32 {
                    continue;
                }
                let n = yy as usize * bw + xx as usize;
                if seeds[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        let tall = y1 - y0 + 1;
        let wide = x1 - x0 + 1;
        // A slot edge runs the full height or width of this patch. Leave the groove.
        if tall * 4 > bh * 3 || wide * 3 > bw {
            for p in pts {
                hole[p] = false;
            }
            continue;
        }
    }
}

/// The reader mask is the core. The pale edge and the soft shadow sit a few
/// pixels outside it. Only pixels that are actually lighter or darker than the
/// surrounding surface are taken in, so the gaps between digits stay.
fn expand_halo(hole: &mut [bool], rgb: &[[u8; 3]], bw: usize, bh: usize) {
    let seed = hole.to_vec();
    let dist = manhattan(&seed, bw, bh, 10);
    for y in 0..bh {
        for x in 0..bw {
            let i = y * bw + x;
            let d = dist[i];
            if !(1..=3).contains(&d) {
                continue;
            }
            let Some(bg) = vertical_surface(rgb, bw, bh, x, y, &|j| dist[j] < 5) else {
                continue;
            };
            let delta = luma_rgb(rgb[i]) - luma_rgb(bg);
            let halo = delta >= 2 || (d == 1 && delta >= -4);
            let shadow = (-50..=-8).contains(&delta);
            if halo || shadow {
                hole[i] = true;
            }
        }
    }
}

fn inpaint_hole(rgb: &mut [[u8; 3]], hole: &[bool], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !hole[i] {
                continue;
            }
            if let Some(color) = vertical_surface(rgb, w, h, x, y, &|j| hole[j]) {
                rgb[i] = color;
            }
        }
    }
}

/// Color of the surface directly above and below the stroke. A slot or a seam
/// sits beside the date, so a square window would mix that groove into the fill.
fn vertical_surface(
    rgb: &[[u8; 3]],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    skip: &impl Fn(usize) -> bool,
) -> Option<[u8; 3]> {
    let gather = |dir: i32| -> (Vec<[u8; 3]>, i32) {
        let mut samples = Vec::with_capacity(4);
        let mut first = 99i32;
        for step in 1..48 {
            let yy = y as i32 + dir * step;
            if yy < 0 || yy >= h as i32 {
                break;
            }
            let j = yy as usize * w + x;
            if skip(j) {
                continue;
            }
            if samples.is_empty() {
                first = step;
            }
            samples.push(rgb[j]);
            if samples.len() == 4 {
                break;
            }
        }
        (samples, first)
    };
    let (mut above, above_at) = gather(-1);
    let (mut below, below_at) = gather(1);
    if above.is_empty() && below.is_empty() {
        return None;
    }
    if above.is_empty() {
        return Some(median_rgb(&mut below));
    }
    if below.is_empty() {
        return Some(median_rgb(&mut above));
    }
    let above_l = luma_rgb(median_rgb(&mut above.clone()));
    let below_l = luma_rgb(median_rgb(&mut below.clone()));
    // The knob is one side and the wall is the other. Keep the nearer side.
    if (above_l - below_l).abs() > 28 {
        if above_at <= below_at {
            Some(median_rgb(&mut above))
        } else {
            Some(median_rgb(&mut below))
        }
    } else {
        above.append(&mut below);
        Some(median_rgb(&mut above))
    }
}

fn median_rgb(samples: &mut [[u8; 3]]) -> [u8; 3] {
    let mid = samples.len() / 2;
    let mut out = [0u8; 3];
    for channel in 0..3 {
        samples.sort_by_key(|px| px[channel]);
        out[channel] = samples[mid][channel];
    }
    out
}

fn manhattan(seed: &[bool], w: usize, h: usize, limit: i16) -> Vec<i16> {
    let mut dist = vec![99i16; seed.len()];
    let mut queue = std::collections::VecDeque::new();
    for (i, on) in seed.iter().enumerate() {
        if *on {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(p) = queue.pop_front() {
        let d = dist[p];
        if d >= limit {
            continue;
        }
        let x = p % w;
        let y = p / w;
        for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
            let xx = x as isize + dx;
            let yy = y as isize + dy;
            if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                continue;
            }
            let n = yy as usize * w + xx as usize;
            if dist[n] > d + 1 {
                dist[n] = d + 1;
                queue.push_back(n);
            }
        }
    }
    dist
}

fn box_blur(rgb: &[[u8; 3]], w: usize, h: usize) -> Vec<[u8; 3]> {
    let mut out = vec![[0u8; 3]; rgb.len()];
    let k = 2i32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for dy in -k..=k {
                let yy = y as i32 + dy;
                if yy < 0 || yy >= h as i32 {
                    continue;
                }
                for dx in -k..=k {
                    let xx = x as i32 + dx;
                    if xx < 0 || xx >= w as i32 {
                        continue;
                    }
                    let px = rgb[yy as usize * w + xx as usize];
                    acc[0] += px[0] as u32;
                    acc[1] += px[1] as u32;
                    acc[2] += px[2] as u32;
                    n += 1;
                }
            }
            out[y * w + x] = [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8];
        }
    }
    out
}

/// The flat fill matches the color but erases grain. Copy the fine detail
/// from a nearby patch of the same surface, above or below the stroke.
fn restore_texture(rgb: &mut [[u8; 3]], hole: &[bool], blur: &[[u8; 3]], w: usize, h: usize) {
    let low = rgb.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !hole[i] {
                continue;
            }
            let target = luma_rgb(low[i]);
            let mut donor = None;
            for step in 6..28 {
                for ny in [y as i32 - step, y as i32 + step] {
                    if ny < 0 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + x;
                    if hole[j] {
                        continue;
                    }
                    if (luma_rgb(low[j]) - luma_rgb(blur[j])).abs() > 24 {
                        continue;
                    }
                    if (luma_rgb(low[j]) - target).abs() <= 18 {
                        donor = Some(j);
                        break;
                    }
                }
                if donor.is_some() {
                    break;
                }
            }
            let Some(j) = donor else {
                continue;
            };
            for channel in 0..3 {
                let mixed =
                    low[i][channel] as i32 + low[j][channel] as i32 - blur[j][channel] as i32;
                rgb[i][channel] = mixed.clamp(0, 255) as u8;
            }
        }
    }
}

fn luma_rgb(px: [u8; 3]) -> i32 {
    (px[0] as i32 * 30 + px[1] as i32 * 59 + px[2] as i32 * 11) / 100
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
    use image::{Rgba, RgbaImage};

    fn assert_only_strokes_change(image: &RgbaImage) {
        let stamp = find_stamp(image).expect("stamp");
        let mut erased = image.clone();
        erase_stamp(&mut erased, &stamp);
        let w = image.width() as i32;
        let h = image.height() as i32;
        let mw = (stamp.x1 - stamp.x0 + 1).max(0) as usize;
        let mut near = vec![false; (w * h) as usize];
        let rad = 8i32;
        let mut ink_n = 0u32;
        let mut ink_same = 0u32;
        for y in stamp.y0..=stamp.y1 {
            for x in stamp.x0..=stamp.x1 {
                let ix = (x - stamp.x0) as usize;
                let iy = (y - stamp.y0) as usize;
                if stamp.ink[iy * mw + ix] == 0 {
                    continue;
                }
                ink_n += 1;
                if image.get_pixel(x as u32, y as u32).0 == erased.get_pixel(x as u32, y as u32).0 {
                    ink_same += 1;
                }
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        let xx = x + dx;
                        let yy = y + dy;
                        if xx >= 0 && yy >= 0 && xx < w && yy < h {
                            near[(yy * w + xx) as usize] = true;
                        }
                    }
                }
            }
        }
        let mut far = 0u32;
        for y in 0..h {
            for x in 0..w {
                if near[(y * w + x) as usize] {
                    continue;
                }
                if image.get_pixel(x as u32, y as u32).0 != erased.get_pixel(x as u32, y as u32).0 {
                    far += 1;
                }
            }
        }
        assert!(ink_n > 40, "ink pixels {ink_n}");
        assert!(
            ink_same * 2 < ink_n,
            "old strokes still in place {ink_same}/{ink_n}"
        );
        assert_eq!(far, 0, "repainted {far} pixels away from the strokes");
    }

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
        assert_only_strokes_change(&before);
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
            CameraTimeValue {
                year: 2026,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                second: None,
            },
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
            CameraTimeValue {
                year: 2023,
                month: 2,
                day: 29,
                hour: 12,
                minute: 0,
                second: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CameraTimeError::Invalid));
        assert_eq!(image.as_raw(), before.as_raw());
    }

    #[test]
    fn light_wall_stamp_with_dots_is_found() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/20261105-0912.jpg");
        let input = crate::image_io::decode_path(&path).expect("panel photo");
        assert_only_strokes_change(&input.rgba);
        let found = detect_camera_time(&input.rgba).expect("date on the light wall");
        assert_eq!(found.text, "2026.11.05 09:12");
        assert_eq!(found.separator, '.');
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
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
    fn cabinet_stamp_cut_by_a_seam_is_found() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/20261105-0912-cabinet.jpg");
        let input = crate::image_io::decode_path(&path).expect("cabinet photo");
        assert_only_strokes_change(&input.rgba);
        let found = detect_camera_time(&input.rgba).expect("date beside the seam");
        assert_eq!(found.text, "2026.11.05 09:12");
        assert_eq!(found.separator, '.');
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
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
        assert_eq!(change.original, "2026.11.05 09:12");
        assert_eq!(change.updated, "2026.12.01 09:30");
        // The cabinet seam still crosses the first 0, so the redrawn glyphs
        // are not always read back. A second read must not invent another time.
        if let Some(again) = detect_camera_time(&image) {
            assert_eq!(again.text, "2026.12.01 09:30");
            assert_eq!(again.separator, '.');
        }
        let mut stray = 0u32;
        for y in 0..image.height() {
            for x in 0..2400 {
                if image.get_pixel(x, y).0 != input.rgba.get_pixel(x, y).0 {
                    stray += 1;
                }
            }
        }
        assert_eq!(stray, 0, "pixels changed on the model line");
    }

    #[test]
    fn white_duct_stamp_is_found() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/20261105-0919-duct.jpg");
        let input = crate::image_io::decode_path(&path).expect("duct photo");
        assert_only_strokes_change(&input.rgba);
        let found = detect_camera_time(&input.rgba).expect("date on the white duct");
        assert_eq!(found.text, "2026.11.05 09:19");
        assert_eq!(found.separator, '.');
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
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
        assert_eq!(change.original, "2026.11.05 09:19");
        assert_eq!(change.updated, "2026.12.01 09:30");
        if let Some(again) = detect_camera_time(&image) {
            assert_eq!(again.text, "2026.12.01 09:30");
            assert_eq!(again.separator, '.');
        }
        let mut stray = 0u32;
        for y in 0..image.height() {
            for x in 0..2400 {
                if image.get_pixel(x, y).0 != input.rgba.get_pixel(x, y).0 {
                    stray += 1;
                }
            }
        }
        assert_eq!(stray, 0, "pixels changed on the model line");
    }

    #[test]
    fn sample_photo_date_can_be_rewritten() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/20261119-092612.jpg");
        let input = crate::image_io::decode_path(&path).expect("sample");
        assert_only_strokes_change(&input.rgba);
        let found = detect_camera_time(&input.rgba).expect("huawei date");
        assert_eq!(found.text, "2026/11/15 16:08");
        assert_eq!(found.second, None);
        let mut image = input.rgba.clone();
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
        let templates =
            crate::template::load_template_dir(&root.join("resources/doubao")).expect("templates");
        let input =
            crate::image_io::decode_path(&root.join("docs/20261119-092612.jpg")).expect("sample");
        let off = crate::restore::process_best(
            &input,
            &templates,
            &crate::restore::ProcessOptions::default(),
        )
        .unwrap();
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
        assert!(!off
            .report
            .flags
            .iter()
            .any(|flag| flag.starts_with("camera_time")));
        assert_eq!(off.report.counts.outside_diff_pixels, 0);
        assert_eq!(on.report.counts.outside_diff_pixels, 0);
        let change = on.report.camera_time.expect("edited");
        assert_eq!(change.original, "2026/11/15 16:08");
        assert_eq!(change.updated, "2026/12/01 09:30");
        assert!(on
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_edited"));
        let off_img = off.image.expect("restored");
        let on_img = on.image.expect("restored with date");
        assert_eq!(
            on_img.get_pixel(900, 1510).0,
            input.rgba.get_pixel(900, 1510).0
        );
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

    #[test]
    fn time_only_leaves_the_picture_except_the_date() {
        let mut marked = painted("2026/11/15 16:08");
        marked.put_pixel(8, 8, Rgba([1, 2, 3, 255]));
        let input = crate::InputImage {
            rgba: marked,
            format: "png".into(),
            precision_reduced: false,
            non_srgb: false,
            exif_tiff: None,
        };
        let out = crate::process_best(
            &input,
            &[],
            &crate::ProcessOptions {
                no_watermark: true,
                camera_time: Some(CameraTimeValue {
                    year: 2026,
                    month: 12,
                    day: 1,
                    hour: 9,
                    minute: 30,
                    second: None,
                }),
                ..crate::ProcessOptions::default()
            },
        )
        .unwrap();
        assert_eq!(out.report.method, "copy");
        assert_eq!(out.report.verdict, crate::Verdict::NeedsReview);
        let change = out.report.camera_time.expect("edited");
        assert_eq!(change.original, "2026/11/15 16:08");
        assert_eq!(change.updated, "2026/12/01 09:30");
        assert!(!out
            .report
            .flags
            .iter()
            .any(|flag| { flag == "camera_time_skipped" || flag == "watermark_not_written" }));
        let image = out.image.expect("time only");
        assert_eq!(image.get_pixel(8, 8).0, [1, 2, 3, 255]);
        let again = detect_camera_time(&image).expect("rewritten");
        assert_eq!(again.text, "2026/12/01 09:30");
    }

    #[test]
    fn time_only_without_a_stamp_does_not_write_a_file() {
        let input = crate::InputImage {
            rgba: RgbaImage::from_pixel(320, 180, Rgba([20, 20, 24, 255])),
            format: "png".into(),
            precision_reduced: false,
            non_srgb: false,
            exif_tiff: None,
        };
        let out = crate::process_best(
            &input,
            &[],
            &crate::ProcessOptions {
                no_watermark: true,
                camera_time: Some(CameraTimeValue {
                    year: 2026,
                    month: 12,
                    day: 1,
                    hour: 9,
                    minute: 30,
                    second: None,
                }),
                ..crate::ProcessOptions::default()
            },
        )
        .unwrap();
        assert!(out.image.is_none());
        assert!(out.encoded.is_none());
        assert!(out.report.output.is_none());
        assert_eq!(out.report.verdict, crate::Verdict::NeedsReview);
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_missing"));
        assert!(!out
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_edited"));
    }

    #[test]
    fn held_watermark_still_writes_the_time_on_the_original() {
        let demo = crate::fixture::demo();
        let mut rgba = demo.watermarked.clone();
        draw_text(
            &mut rgba,
            "2026/11/15 16:08",
            220,
            80,
            32.0,
            [255, 255, 255],
        );
        let mut template = demo.template.clone();
        template.meta.ncc_auto = 1.01;
        template.meta.ncc_confirm = 0.0;
        let input = crate::InputImage {
            rgba: rgba.clone(),
            format: "png".into(),
            precision_reduced: false,
            non_srgb: false,
            exif_tiff: None,
        };
        let out = crate::process_one(
            &input,
            &template,
            &crate::ProcessOptions {
                camera_time: Some(CameraTimeValue {
                    year: 2026,
                    month: 12,
                    day: 1,
                    hour: 9,
                    minute: 30,
                    second: None,
                }),
                ..crate::ProcessOptions::default()
            },
        )
        .unwrap();
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "confirmation_required"));
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "watermark_not_written"));
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_edited"));
        assert!(!out
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_skipped"));
        let image = out.image.expect("time file");
        let change = out.report.camera_time.expect("edited");
        assert_eq!(change.updated, "2026/12/01 09:30");
        let [x, y, w, h] = out.report.bbox.expect("box");
        let mut kept = false;
        for py in y..y + h {
            for px in x..x + w {
                let before = rgba.get_pixel(px as u32, py as u32).0;
                if before != demo.clean.get_pixel(px as u32, py as u32).0 {
                    assert_eq!(
                        image.get_pixel(px as u32, py as u32).0,
                        before,
                        "watermark pixel {px},{py}"
                    );
                    kept = true;
                }
            }
        }
        assert!(kept, "expected an untouched watermark pixel");
    }

    #[test]
    fn missing_stamp_on_a_held_watermark_does_not_emit_a_copy() {
        let demo = crate::fixture::demo();
        let mut template = demo.template.clone();
        template.meta.ncc_auto = 1.01;
        template.meta.ncc_confirm = 0.0;
        let input = crate::InputImage {
            rgba: demo.watermarked,
            format: "png".into(),
            precision_reduced: false,
            non_srgb: false,
            exif_tiff: None,
        };
        let out = crate::process_one(
            &input,
            &template,
            &crate::ProcessOptions {
                camera_time: Some(CameraTimeValue {
                    year: 2026,
                    month: 12,
                    day: 1,
                    hour: 9,
                    minute: 30,
                    second: None,
                }),
                ..crate::ProcessOptions::default()
            },
        )
        .unwrap();
        assert!(out.image.is_none());
        assert!(out.encoded.is_none());
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "camera_time_missing"));
        assert!(out
            .report
            .flags
            .iter()
            .any(|flag| flag == "confirmation_required"));
        assert!(!out
            .report
            .flags
            .iter()
            .any(|flag| flag == "watermark_not_written"));
    }
}
