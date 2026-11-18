//! PSNR and SSIM for holdout images that still have a clean original.
//! Runtime reports do not invent these numbers.

pub fn psnr_from_mse(mse: f64) -> f64 {
    if mse <= 1e-12 {
        99.0
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

pub fn psnr_rgb(clean: &[u8], restored: &[u8]) -> f64 {
    debug_assert_eq!(clean.len(), restored.len());
    if clean.is_empty() {
        return 99.0;
    }
    let mut acc = 0.0f64;
    for (a, b) in clean.iter().zip(restored.iter()) {
        let d = *a as f64 - *b as f64;
        acc += d * d;
    }
    psnr_from_mse(acc / clean.len() as f64)
}

/// Wang 2004, 11×11 Gaussian σ=1.5, K1=0.01, K2=0.03, L=255.
/// Returns the minimum of the three channels. Windows that do not fit
/// yield 1 when the crop is identical and 0 otherwise.
pub fn ssim_min_rgb(clean: &[u8], restored: &[u8], width: u32, height: u32) -> f64 {
    debug_assert_eq!(clean.len(), restored.len());
    debug_assert_eq!(clean.len(), (width * height * 3) as usize);
    if width < 11 || height < 11 {
        let same = clean == restored;
        return if same { 1.0 } else { 0.0 };
    }
    let kernel = gaussian_window();
    let mut min_c = 1.0f64;
    for channel in 0..3 {
        let mut a = vec![0.0f32; (width * height) as usize];
        let mut b = vec![0.0f32; (width * height) as usize];
        for i in 0..(width * height) as usize {
            a[i] = clean[i * 3 + channel] as f32;
            b[i] = restored[i * 3 + channel] as f32;
        }
        min_c = min_c.min(ssim_plane(&a, &b, width, height, &kernel));
    }
    min_c
}

fn gaussian_window() -> [f64; 121] {
    let sigma = 1.5f64;
    let mut k = [0.0f64; 121];
    let mut sum = 0.0;
    for y in 0..11 {
        for x in 0..11 {
            let dx = x as f64 - 5.0;
            let dy = y as f64 - 5.0;
            let v = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
            k[y * 11 + x] = v;
            sum += v;
        }
    }
    for v in &mut k {
        *v /= sum;
    }
    k
}

fn ssim_plane(a: &[f32], b: &[f32], width: u32, height: u32, kernel: &[f64; 121]) -> f64 {
    let c1 = (0.01f64 * 255.0) * (0.01f64 * 255.0);
    let c2 = (0.03f64 * 255.0) * (0.03f64 * 255.0);
    let mut acc = 0.0f64;
    let mut n = 0u64;
    for y in 0..=height - 11 {
        for x in 0..=width - 11 {
            let mut mu1 = 0.0f64;
            let mut mu2 = 0.0f64;
            let mut m11 = 0.0f64;
            let mut m22 = 0.0f64;
            let mut m12 = 0.0f64;
            for ky in 0..11u32 {
                for kx in 0..11u32 {
                    let w = kernel[(ky * 11 + kx) as usize];
                    let p = a[((y + ky) * width + (x + kx)) as usize] as f64;
                    let q = b[((y + ky) * width + (x + kx)) as usize] as f64;
                    mu1 += w * p;
                    mu2 += w * q;
                    m11 += w * p * p;
                    m22 += w * q * q;
                    m12 += w * p * q;
                }
            }
            let v1 = (m11 - mu1 * mu1).max(0.0);
            let v2 = (m22 - mu2 * mu2).max(0.0);
            let v12 = m12 - mu1 * mu2;
            let num = (2.0 * mu1 * mu2 + c1) * (2.0 * v12 + c2);
            let den = (mu1 * mu1 + mu2 * mu2 + c1) * (v1 + v2 + c2);
            acc += num / den;
            n += 1;
        }
    }
    if n == 0 {
        1.0
    } else {
        acc / n as f64
    }
}

pub fn max_abs(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_scores() {
        let buf = vec![40u8, 80, 120, 10, 20, 30];
        assert_eq!(psnr_rgb(&buf, &buf), 99.0);
        assert_eq!(max_abs(&buf, &buf), 0);
        let side = 32u32;
        let n = (side * side * 3) as usize;
        let img = vec![90u8; n];
        assert!(ssim_min_rgb(&img, &img, side, side) > 0.999);
    }

    #[test]
    fn ssim_drops_when_channels_differ() {
        let side = 32u32;
        let n = (side * side * 3) as usize;
        let a = vec![20u8; n];
        let mut b = a.clone();
        for px in b.chunks_exact_mut(3) {
            px[0] = 220;
        }
        assert!(ssim_min_rgb(&a, &b, side, side) < 0.5);
    }
}
