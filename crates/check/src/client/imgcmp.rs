//! Image comparison for the client visual-parity harness: PNG decode/encode, box downscale,
//! per-tile SSIM and colour-histogram distance, diff and side-by-side images.
//! Pure functions on 8-bit RGB buffers so they can be unit tested without a GPU.
use anyhow::{bail, Context, Result};
use std::path::Path;

#[derive(Clone)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Img {
    pub fn new(w: usize, h: usize) -> Img {
        Img {
            w,
            h,
            rgb: vec![0; w * h * 3],
        }
    }

    pub fn load(p: &Path) -> Result<Img> {
        let f = std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(f));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = dec.read_info()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf)?;
        let (w, h) = (info.width as usize, info.height as usize);
        let ch = info.color_type.samples();
        let mut rgb = Vec::with_capacity(w * h * 3);
        for px in buf[..info.buffer_size()].chunks(ch) {
            match info.color_type {
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => {
                    rgb.extend([px[0], px[0], px[0]])
                }
                png::ColorType::Rgb | png::ColorType::Rgba => rgb.extend(&px[..3]),
                png::ColorType::Indexed => bail!("indexed png not supported"),
            }
        }
        Ok(Img { w, h, rgb })
    }

    pub fn save(&self, p: &Path) -> Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let f = std::fs::File::create(p)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&self.rgb)?;
        Ok(())
    }

    /// Rows `y0..` only (drops a window title bar).
    pub fn crop_top(&self, y0: usize) -> Img {
        let y0 = y0.min(self.h);
        Img {
            w: self.w,
            h: self.h - y0,
            rgb: self.rgb[y0 * self.w * 3..].to_vec(),
        }
    }

    /// Area-average resample to w x h.
    pub fn resize(&self, w: usize, h: usize) -> Img {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut o = Img::new(w, h);
        for y in 0..h {
            let y0 = y * self.h / h;
            let y1 = ((y + 1) * self.h).div_ceil(h).max(y0 + 1).min(self.h);
            for x in 0..w {
                let x0 = x * self.w / w;
                let x1 = ((x + 1) * self.w).div_ceil(w).max(x0 + 1).min(self.w);
                let mut s = [0u32; 3];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        for (c, v) in s.iter_mut().enumerate() {
                            *v += self.rgb[(yy * self.w + xx) * 3 + c] as u32;
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                for (c, v) in s.iter().enumerate() {
                    o.rgb[(y * w + x) * 3 + c] = (v / n) as u8;
                }
            }
        }
        o
    }

    fn luma(&self) -> Vec<f32> {
        self.rgb
            .chunks(3)
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect()
    }
}

type Rect = (usize, usize, usize, usize);

/// Mean SSIM of the luma over the rectangle (x0,y0,x1,y1), 7x7 uniform windows with stride 3.
fn ssim_rect(a: &[f32], b: &[f32], w: usize, r: Rect) -> f64 {
    const WIN: usize = 7;
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (x0, y0, x1, y1) = r;
    let (mut sum, mut n) = (0.0f64, 0usize);
    let mut y = y0;
    while y + WIN <= y1 {
        let mut x = x0;
        while x + WIN <= x1 {
            let (mut ma, mut mb) = (0.0f64, 0.0f64);
            for yy in y..y + WIN {
                for xx in x..x + WIN {
                    ma += a[yy * w + xx] as f64;
                    mb += b[yy * w + xx] as f64;
                }
            }
            let k = (WIN * WIN) as f64;
            ma /= k;
            mb /= k;
            let (mut va, mut vb, mut cov) = (0.0f64, 0.0f64, 0.0f64);
            for yy in y..y + WIN {
                for xx in x..x + WIN {
                    let (da, db) = (a[yy * w + xx] as f64 - ma, b[yy * w + xx] as f64 - mb);
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= k - 1.0;
            vb /= k - 1.0;
            cov /= k - 1.0;
            sum += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += 3;
        }
        y += 3;
    }
    if n == 0 {
        1.0
    } else {
        sum / n as f64
    }
}

/// Colour histogram distance on a rectangle: half the L1 distance between the normalised
/// 16-bin per-channel histograms, averaged over the channels (0 = identical, 1 = disjoint).
fn hist_rect(a: &Img, b: &Img, r: Rect) -> f64 {
    let (x0, y0, x1, y1) = r;
    let mut h = [[[0f64; 16]; 3]; 2];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            for c in 0..3 {
                h[0][c][(a.rgb[(y * a.w + x) * 3 + c] >> 4) as usize] += 1.0;
                h[1][c][(b.rgb[(y * b.w + x) * 3 + c] >> 4) as usize] += 1.0;
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    (0..3)
        .map(|c| {
            0.5 * (0..16)
                .map(|i| (h[0][c][i] - h[1][c][i]).abs() / n)
                .sum::<f64>()
        })
        .sum::<f64>()
        / 3.0
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub col: usize,
    pub row: usize,
    pub ssim: f64,
    pub hist: f64,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub tiles: Vec<Tile>,
    pub cols: usize,
    pub rows: usize,
}

impl Report {
    pub fn mean_ssim(&self) -> f64 {
        self.tiles.iter().map(|t| t.ssim).sum::<f64>() / self.tiles.len().max(1) as f64
    }
    pub fn min_ssim(&self) -> f64 {
        self.tiles.iter().map(|t| t.ssim).fold(1.0, f64::min)
    }
    pub fn mean_hist(&self) -> f64 {
        self.tiles.iter().map(|t| t.hist).sum::<f64>() / self.tiles.len().max(1) as f64
    }
    pub fn max_hist(&self) -> f64 {
        self.tiles.iter().map(|t| t.hist).fold(0.0, f64::max)
    }
}

/// Compare two images (resized to `cmp_w` x `cmp_h` first) on a cols x rows tile grid.
pub fn compare(a: &Img, b: &Img, cols: usize, rows: usize, cmp_w: usize, cmp_h: usize) -> Report {
    let (a, b) = (a.resize(cmp_w, cmp_h), b.resize(cmp_w, cmp_h));
    let (la, lb) = (a.luma(), b.luma());
    let mut tiles = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let r = (
                col * cmp_w / cols,
                row * cmp_h / rows,
                (col + 1) * cmp_w / cols,
                (row + 1) * cmp_h / rows,
            );
            tiles.push(Tile {
                col,
                row,
                ssim: ssim_rect(&la, &lb, cmp_w, r),
                hist: hist_rect(&a, &b, r),
            });
        }
    }
    Report { tiles, cols, rows }
}

/// Absolute per-pixel difference (x4) with the tiles that exceed the given limits outlined in red.
pub fn diff_image(a: &Img, b: &Img, rep: &Report, min_ssim: f64, max_hist: f64) -> Img {
    let a = a.resize(b.w, b.h);
    let mut o = Img::new(a.w, a.h);
    for i in 0..a.rgb.len() {
        o.rgb[i] = (a.rgb[i].abs_diff(b.rgb[i]) as u32 * 4).min(255) as u8;
    }
    for t in rep
        .tiles
        .iter()
        .filter(|t| t.ssim < min_ssim || t.hist > max_hist)
    {
        let (x0, y0, x1, y1) = (
            t.col * o.w / rep.cols,
            t.row * o.h / rep.rows,
            (t.col + 1) * o.w / rep.cols,
            (t.row + 1) * o.h / rep.rows,
        );
        for x in x0..x1 {
            for y in [y0, y1 - 1] {
                o.rgb[(y * o.w + x) * 3..][..3].copy_from_slice(&[255, 0, 0]);
            }
        }
        for y in y0..y1 {
            for x in [x0, x1 - 1] {
                o.rgb[(y * o.w + x) * 3..][..3].copy_from_slice(&[255, 0, 0]);
            }
        }
    }
    o
}

/// Vanilla | Basalt | diff, side by side.
pub fn side_by_side(a: &Img, b: &Img, d: &Img) -> Img {
    let h = a.h.max(b.h).max(d.h);
    let mut o = Img::new(a.w + b.w + d.w + 8, h);
    let mut x = 0;
    for im in [a, b, d] {
        for y in 0..im.h {
            o.rgb[(y * o.w + x) * 3..(y * o.w + x + im.w) * 3]
                .copy_from_slice(&im.rgb[y * im.w * 3..(y + 1) * im.w * 3]);
        }
        x += im.w + 4;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    fn noise(w: usize, h: usize, seed: u32) -> Img {
        let mut s = seed;
        let mut i = Img::new(w, h);
        for v in i.rgb.iter_mut() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (s >> 24) as u8;
        }
        i
    }
    #[test]
    fn identical_is_perfect() {
        let a = noise(64, 48, 1);
        let r = compare(&a, &a, 4, 3, 64, 48);
        assert!(r.min_ssim() > 0.999 && r.max_hist() < 1e-9);
    }
    #[test]
    fn different_is_low() {
        let r = compare(&noise(64, 48, 1), &noise(64, 48, 2), 4, 3, 64, 48);
        assert!(r.mean_ssim() < 0.2 && r.mean_hist() < 0.3);
        let black = Img::new(64, 48);
        let mut white = Img::new(64, 48);
        white.rgb.fill(255);
        assert!(compare(&black, &white, 2, 2, 64, 48).max_hist() > 0.99);
    }
    #[test]
    fn resize_keeps_mean() {
        let mut a = Img::new(8, 8);
        a.rgb.fill(100);
        assert!(a.resize(4, 4).rgb.iter().all(|&v| v == 100));
        assert!(a.resize(16, 16).rgb.iter().all(|&v| v == 100));
    }
}
