use crate::Error;
use image::{ImageBuffer, ImageEncoder, Rgba, RgbaImage};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct InputImage {
    pub rgba: RgbaImage,
    pub format: String,
    pub precision_reduced: bool,
    pub non_srgb: bool,
    pub exif_tiff: Option<Vec<u8>>,
}

pub fn decode_bytes(bytes: &[u8]) -> Result<InputImage, Error> {
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Err(Error::UnsupportedFormat);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Err(Error::UnsupportedFormat);
    }
    let info = inspect(bytes);
    if info.indexed || info.animated {
        return Err(Error::UnsupportedFormat);
    }
    let dynimg = image::load_from_memory(bytes).map_err(|e| Error::Decode(e.to_string()))?;
    let precision_reduced = matches!(
        dynimg,
        image::DynamicImage::ImageLuma16(_)
            | image::DynamicImage::ImageLumaA16(_)
            | image::DynamicImage::ImageRgb16(_)
            | image::DynamicImage::ImageRgba16(_)
    );
    let rgba = dynimg.to_rgba8();
    let rgba = apply_orientation(rgba, info.orientation);
    Ok(InputImage {
        rgba,
        format: info.format,
        precision_reduced,
        non_srgb: info.non_srgb,
        exif_tiff: info.exif_tiff,
    })
}

pub fn decode_path(path: &std::path::Path) -> Result<InputImage, Error> {
    let bytes = std::fs::read(path)?;
    decode_bytes(&bytes)
}

#[derive(Debug)]
struct Inspected {
    format: String,
    orientation: u16,
    non_srgb: bool,
    indexed: bool,
    animated: bool,
    exif_tiff: Option<Vec<u8>>,
}

fn inspect(bytes: &[u8]) -> Inspected {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        inspect_png(bytes)
    } else if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        inspect_jpeg(bytes)
    } else {
        Inspected {
            format: "unknown".into(),
            orientation: 1,
            non_srgb: false,
            indexed: false,
            animated: false,
            exif_tiff: None,
        }
    }
}

fn inspect_png(bytes: &[u8]) -> Inspected {
    let format = "png".to_string();
    let mut orientation = 1u16;
    let mut non_srgb = false;
    let mut indexed = false;
    let mut animated = false;
    let mut exif_tiff = None;
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let kind = &bytes[i + 4..i + 8];
        let start = i + 8;
        let end = start + len;
        if end + 4 > bytes.len() {
            break;
        }
        let data = &bytes[start..end];
        match kind {
            b"IHDR" if data.len() >= 10 => {
                let color = data[9];
                indexed = color == 3;
                let _ = format;
            }
            b"acTL" => animated = true,
            b"iCCP" => {
                if let Some(name_end) = data.iter().position(|b| *b == 0) {
                    let name = String::from_utf8_lossy(&data[..name_end]).to_ascii_lowercase();
                    if !name.contains("srgb") {
                        non_srgb = true;
                    }
                }
            }
            b"eXIf" => {
                orientation = tiff_orientation(data).unwrap_or(1);
                exif_tiff = Some(data.to_vec());
            }
            _ => {}
        }
        i = end + 4;
    }
    Inspected {
        format,
        orientation,
        non_srgb,
        indexed,
        animated,
        exif_tiff,
    }
}

fn inspect_jpeg(bytes: &[u8]) -> Inspected {
    let mut orientation = 1u16;
    let mut exif_tiff = None;
    let mut non_srgb = false;
    let mut saw_icc = false;
    let mut i = 2usize;
    while i + 4 < bytes.len() {
        if bytes[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 4 > bytes.len() {
            break;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        let payload = &bytes[i + 4..i + 2 + len];
        if marker == 0xe1 && payload.len() >= 6 && &payload[..6] == b"Exif\0\0" {
            let tiff = &payload[6..];
            orientation = tiff_orientation(tiff).unwrap_or(1);
            exif_tiff = Some(tiff.to_vec());
        }
        if marker == 0xe2 && payload.windows(11).any(|w| w == b"ICC_PROFILE") {
            saw_icc = true;
        }
        i += 2 + len;
    }
    if saw_icc && !bytes.windows(4).any(|w| w.eq_ignore_ascii_case(b"sRGB")) {
        non_srgb = true;
    }
    Inspected {
        format: "jpeg".into(),
        orientation,
        non_srgb,
        indexed: false,
        animated: false,
        exif_tiff,
    }
}

fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    if tiff.len() < 8 {
        return None;
    }
    let le = match &tiff[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |o: usize| -> Option<u16> {
        let b = tiff.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let read_u32 = |o: usize| -> Option<u32> {
        let b = tiff.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    if read_u16(2)? != 42 {
        return None;
    }
    let mut ifd = read_u32(4)? as usize;
    if ifd + 2 > tiff.len() {
        return None;
    }
    let count = read_u16(ifd)? as usize;
    ifd += 2;
    for _ in 0..count {
        if ifd + 12 > tiff.len() {
            return None;
        }
        let tag = read_u16(ifd)?;
        let typ = read_u16(ifd + 2)?;
        let n = read_u32(ifd + 4)?;
        if tag == 0x0112 && typ == 3 && n == 1 {
            return read_u16(ifd + 8);
        }
        ifd += 12;
    }
    None
}

pub fn apply_orientation(src: RgbaImage, orientation: u16) -> RgbaImage {
    match orientation {
        2 => flip_h(src),
        3 => rot180(src),
        4 => flip_v(src),
        5 => rot90cw(flip_h(src)),
        6 => rot90cw(src),
        7 => rot90cw(flip_v(src)),
        8 => rot270cw(src),
        _ => src,
    }
}

fn flip_h(src: RgbaImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let mut dst = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            dst.put_pixel(w - 1 - x, y, *src.get_pixel(x, y));
        }
    }
    dst
}

fn flip_v(src: RgbaImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let mut dst = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            dst.put_pixel(x, h - 1 - y, *src.get_pixel(x, y));
        }
    }
    dst
}

fn rot180(src: RgbaImage) -> RgbaImage {
    flip_h(flip_v(src))
}

pub fn rot90cw(src: RgbaImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let mut dst = RgbaImage::new(h, w);
    for y in 0..h {
        for x in 0..w {
            dst.put_pixel(h - 1 - y, x, *src.get_pixel(x, y));
        }
    }
    dst
}

fn rot270cw(src: RgbaImage) -> RgbaImage {
    rot90cw(rot90cw(rot90cw(src)))
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, Error> {
    let mut cursor = Cursor::new(Vec::new());
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut cursor,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    img.write_with_encoder(encoder)
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(cursor.into_inner())
}

pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, Error> {
    let (w, h) = img.dimensions();
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for (i, px) in img.pixels().enumerate() {
        let a = px.0[3] as f32 / 255.0;
        for c in 0..3 {
            let v = px.0[c] as f32 * a + 255.0 * (1.0 - a);
            rgb[i * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    let mut cursor = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
    encoder
        .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(cursor.into_inner())
}

pub fn patch_exif_orientation_to_top_left(tiff: &mut [u8]) -> bool {
    if tiff.len() < 8 {
        return false;
    }
    let le = match &tiff[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return false,
    };
    let read_u16 = |buf: &[u8], o: usize| -> Option<u16> {
        let b = buf.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let read_u32 = |buf: &[u8], o: usize| -> Option<u32> {
        let b = buf.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    if read_u16(tiff, 2) != Some(42) {
        return false;
    }
    let mut ifd = match read_u32(tiff, 4) {
        Some(v) => v as usize,
        None => return false,
    };
    let count = match read_u16(tiff, ifd) {
        Some(v) => v as usize,
        None => return false,
    };
    ifd += 2;
    for _ in 0..count {
        if ifd + 12 > tiff.len() {
            return false;
        }
        let tag = match read_u16(tiff, ifd) {
            Some(v) => v,
            None => return false,
        };
        if tag == 0x0112 {
            if le {
                tiff[ifd + 8] = 1;
                tiff[ifd + 9] = 0;
            } else {
                tiff[ifd + 8] = 0;
                tiff[ifd + 9] = 1;
            }
            return true;
        }
        ifd += 12;
    }
    false
}

pub fn png_insert_chunk(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Error> {
    let iend = png
        .windows(8)
        .position(|w| w[4..8] == *b"IEND")
        .ok_or_else(|| Error::Decode("PNG 缺少 IEND".into()))?;
    // windows position is the length field of IEND.
    let mut out = Vec::with_capacity(png.len() + data.len() + 12);
    out.extend_from_slice(&png[..iend]);
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_data = Vec::with_capacity(4 + data.len());
    crc_data.extend_from_slice(kind);
    crc_data.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    out.extend_from_slice(&png[iend..]);
    Ok(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

pub fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
    ImageBuffer::from_fn(w, h, |_, _| Rgba([rgb[0], rgb[1], rgb[2], 255]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_orientation_bottom_right() {
        let (w, h) = (80u32, 60u32);
        let mut stored = solid(w, h, [0, 0, 0]);
        stored.put_pixel(w - 1, 0, Rgba([255, 255, 255, 255]));
        stored.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        let baked = apply_orientation(stored, 6);
        assert_eq!(baked.dimensions(), (h, w));
        let bottom_right = baked.get_pixel(h - 1, w - 1).0;
        assert_eq!(&bottom_right[..3], &[255, 255, 255]);
        let top_right = baked.get_pixel(h - 1, 0).0;
        assert_eq!(&top_right[..3], &[0, 0, 0]);

        let mut png = encode_png(&{
            let (w, h) = (32u32, 24u32);
            let mut img = solid(w, h, [10, 20, 30]);
            img.put_pixel(w - 1, 0, Rgba([240, 10, 10, 255]));
            img
        })
        .unwrap();
        let tiff = orientation_tiff(6);
        png = png_insert_chunk(&png, b"eXIf", &tiff).unwrap();
        let decoded = decode_bytes(&png).unwrap();
        assert_eq!(decoded.rgba.dimensions(), (24, 32));
        let px = decoded.rgba.get_pixel(23, 31).0;
        assert!(px[0] > 200 && px[1] < 40, "visual bottom-right should be the stored top-right, got {px:?}");
    }

    fn orientation_tiff(orientation: u16) -> Vec<u8> {
        let mut t = vec![0u8; 26];
        t[0] = b'I';
        t[1] = b'I';
        t[2] = 42;
        t[4] = 8;
        t[8] = 1;
        t[10] = 0x12;
        t[11] = 0x01;
        t[12] = 3;
        t[14] = 1;
        t[18] = orientation as u8;
        t[19] = (orientation >> 8) as u8;
        t
    }

    #[test]
    fn gif_is_rejected() {
        let err = decode_bytes(b"GIF89a").unwrap_err();
        assert!(matches!(err, Error::UnsupportedFormat));
    }
}
