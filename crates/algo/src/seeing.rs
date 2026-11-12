//! Observation aids: view-only transforms that show one visual property of a picture at a time
//! (its tones, its colour masses, its big shapes, its lines, its colour without tone), and
//! seeded random frames for studying a small part of it.
//!
//! They are for looking, not for editing: each takes a straight-alpha `f32` RGBA buffer (a
//! composite, usually already reduced to preview size) and rewrites it in place. Nothing here
//! judges a picture or "improves" it. The same input and parameters always give the same
//! output; the only randomness is [`frames`], which is a pure function of its seed.
//!
//! Luminosity is [`magicpic_color::blend::lum`], the editor's own definition (the one the
//! Luminosity blend mode and the histogram use), taken on the composite's encoded values. It is
//! an aid for comparing tones, not a colorimetric measurement.

use magicpic_color::blend::{lum, set_lum};
use serde::{Deserialize, Serialize};

/// Largest buffer a view accepts (a 4096² preview). Callers reduce the composite first.
pub const MAX_VIEW_PIXELS: usize = 4096 * 4096;
/// Most frames one [`frames`] call returns.
pub const MAX_FRAMES: usize = 64;

/// What a view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    /// Tones only: every pixel as the grey of its luminosity.
    Luminance,
    /// Colour masses: the picture strongly defocused, so detail goes and the colour areas stay.
    Masses,
    /// Big shapes: a few flat tones (a notan), so figure and empty space read as shapes.
    Shapes,
    /// Lines: outlines as dark strokes on white, surface texture suppressed.
    Lines,
    /// Colour without tone: every pixel at the same luminosity, hue and saturation kept.
    Color,
}

impl View {
    pub const ALL: [View; 5] = [View::Luminance, View::Masses, View::Shapes, View::Lines, View::Color];

    pub fn id(self) -> &'static str {
        match self {
            View::Luminance => "luminance",
            View::Masses => "masses",
            View::Shapes => "shapes",
            View::Lines => "lines",
            View::Color => "color",
        }
    }

    pub fn from_id(s: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.id() == s)
    }

    /// One line on what the view is for, shown to people and agents.
    pub fn describe(self) -> &'static str {
        match self {
            View::Luminance => "tones only (grey of each pixel's luminosity): where the light and dark areas are, without colour",
            View::Masses => "colour masses (strong defocus): the big areas of colour, without detail",
            View::Shapes => "big shapes (a few flat tones): figure against empty space",
            View::Lines => "lines (outlines on white): edges and directions, without surface texture",
            View::Color => "colour without tone (equal luminosity): hue and saturation relationships",
        }
    }
}

/// Why a view or frame request was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeeingError {
    /// Width or height is zero, or the buffer length is not width × height.
    BadBuffer,
    /// More than [`MAX_VIEW_PIXELS`] pixels.
    TooLarge,
    /// A parameter is outside its range; the text names it.
    BadParam(&'static str),
}

impl std::fmt::Display for SeeingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeeingError::BadBuffer => f.write_str("seeing view: buffer does not match its width and height"),
            SeeingError::TooLarge => write!(f, "seeing view: more than {MAX_VIEW_PIXELS} pixels; reduce the preview size"),
            SeeingError::BadParam(what) => write!(f, "seeing: {what}"),
        }
    }
}

impl std::error::Error for SeeingError {}

/// Parameters of a view. `strength` (0–1, default 0.5) is how far the view abstracts: the
/// defocus radius for [`View::Masses`], the pre-blur for [`View::Shapes`] and [`View::Lines`].
/// `levels` (2–8, default 3) is the number of flat tones of [`View::Shapes`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewParams {
    pub strength: f32,
    pub levels: u32,
}

impl Default for ViewParams {
    fn default() -> Self {
        Self { strength: 0.5, levels: 3 }
    }
}

impl ViewParams {
    fn check(&self) -> Result<(), SeeingError> {
        if !(self.strength.is_finite() && (0.0..=1.0).contains(&self.strength)) {
            return Err(SeeingError::BadParam("strength must be between 0 and 1"));
        }
        if !(2..=8).contains(&self.levels) {
            return Err(SeeingError::BadParam("levels must be between 2 and 8"));
        }
        Ok(())
    }
}

/// Rewrite `px` (`w` × `h`, straight-alpha RGBA, row-major) as `view` of itself. Alpha is kept
/// (blurred with the colour for the defocused views), so transparency stays transparency; only
/// [`View::Lines`] is opaque, a drawing on paper.
pub fn apply_view(view: View, params: ViewParams, w: usize, h: usize, px: &mut [[f32; 4]]) -> Result<(), SeeingError> {
    params.check()?;
    let n = w.checked_mul(h).ok_or(SeeingError::BadBuffer)?;
    if w == 0 || h == 0 || px.len() != n {
        return Err(SeeingError::BadBuffer);
    }
    if n > MAX_VIEW_PIXELS {
        return Err(SeeingError::TooLarge);
    }
    // Non-finite samples (32-bit documents) would poison a blur; treat them as black.
    for p in px.iter_mut() {
        for v in p.iter_mut() {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        p[3] = p[3].clamp(0.0, 1.0);
    }
    let long = w.max(h) as f32;
    match view {
        View::Luminance => {
            for p in px.iter_mut() {
                let l = lum([p[0], p[1], p[2]]).clamp(0.0, 1.0);
                *p = [l, l, l, p[3]];
            }
        }
        View::Masses => {
            // 1 % to 5 % of the long side: from "squinting" to fully defocused.
            let radius = long * (0.01 + 0.04 * params.strength);
            blur(w, h, px, radius);
        }
        View::Shapes => {
            blur(w, h, px, long * (0.002 + 0.012 * params.strength));
            let (lo, hi) = tone_range(px);
            let levels = params.levels as f32;
            for p in px.iter_mut() {
                let l = lum([p[0], p[1], p[2]]);
                let t = if hi > lo { ((l - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.5 };
                // Equal bands of the picture's own tonal range, each shown as its middle grey.
                let band = (t * levels).floor().min(levels - 1.0);
                let g = if hi > lo { band / (levels - 1.0) } else { l.clamp(0.0, 1.0) };
                *p = [g, g, g, p[3]];
            }
        }
        View::Lines => {
            blur(w, h, px, long * (0.001 + 0.006 * params.strength));
            let l: Vec<f32> = px.iter().map(|p| lum([p[0], p[1], p[2]]) * p[3] + (1.0 - p[3])).collect();
            let at = |x: usize, y: usize| l.get(y * w + x).copied().unwrap_or(0.0);
            let mut mag = vec![0.0f32; n];
            for y in 0..h {
                let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
                for x in 0..w {
                    let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
                    // Sobel, edges replicated.
                    let gx = (at(x1, y0) + 2.0 * at(x1, y) + at(x1, y1)) - (at(x0, y0) + 2.0 * at(x0, y) + at(x0, y1));
                    let gy = (at(x0, y1) + 2.0 * at(x, y1) + at(x1, y1)) - (at(x0, y0) + 2.0 * at(x, y0) + at(x1, y0));
                    if let Some(slot) = mag.get_mut(y * w + x) {
                        *slot = (gx * gx + gy * gy).sqrt();
                    }
                }
            }
            // Ink starts where an edge stands out against the picture's own strong edges (the
            // 99.5th percentile, so one specular pixel does not set the scale), and never on more
            // than the strongest fifth of the picture: outlines draw, surface texture and grain
            // stay paper, and an evenly patterned picture shows its pattern as lines instead of
            // going black. A flat picture stays white.
            let (common, strong) = edge_levels(&mag);
            let lo = (0.15 * strong).max(common);
            // Full ink at 70 % of a strong edge, or at the strong edge itself when the common
            // level is already close to it (an even pattern).
            let hi = (0.7 * strong).max(lo * 1.25).min(strong).max(lo + f32::EPSILON);
            for (p, m) in px.iter_mut().zip(&mag) {
                let u = if strong > 1e-4 { ((m - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
                let ink = u * u * (3.0 - 2.0 * u);
                let g = 1.0 - ink;
                // Lines are drawn on paper: the view is opaque, and the edge of a shape against
                // transparency is one of its lines.
                *p = [g, g, g, 1.0];
            }
        }
        View::Color => {
            for p in px.iter_mut() {
                let c = set_lum([p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0)], 0.5);
                *p = [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), p[3]];
            }
        }
    }
    Ok(())
}

/// The edge strengths [`View::Lines`] scales by: the 80th percentile of all magnitudes (what a
/// fifth of the picture exceeds) and the 99.5th percentile of the non-zero ones (its strong
/// edges), from a 4096-bin histogram up to the largest. Both 0 for a picture without edges.
fn edge_levels(mag: &[f32]) -> (f32, f32) {
    const BINS: usize = 4096;
    let peak = mag.iter().copied().fold(0.0f32, f32::max);
    if !(peak.is_finite() && peak > 0.0) {
        return (0.0, 0.0);
    }
    let mut hist = vec![0u64; BINS];
    let mut edges = 0u64;
    for m in mag {
        let bin = ((m.max(0.0) / peak * (BINS - 1) as f32) as usize).min(BINS - 1);
        if let Some(slot) = hist.get_mut(bin) {
            *slot += 1;
        }
        if *m > 0.0 {
            edges += 1;
        }
    }
    let value = |bin: usize| (bin as f32 + 0.5) / BINS as f32 * peak;
    // Walking down from the strongest: the bin where `count` pixels have been passed.
    let from_top = |count: u64| -> f32 {
        let mut acc = 0u64;
        for (i, c) in hist.iter().enumerate().rev() {
            acc += c;
            if acc > count {
                return value(i);
            }
        }
        0.0
    };
    (from_top(mag.len() as u64 / 5), from_top(edges / 200))
}

/// The 2nd and 98th percentile of the visible pixels' luminosity (a 1024-bin histogram), so a
/// few specular or black pixels do not set the bands of [`View::Shapes`].
fn tone_range(px: &[[f32; 4]]) -> (f32, f32) {
    const BINS: usize = 1024;
    let mut hist = [0u64; BINS];
    let mut total = 0u64;
    for p in px {
        if p[3] <= 0.0 {
            continue;
        }
        let l = lum([p[0], p[1], p[2]]).clamp(0.0, 1.0);
        let bin = ((l * (BINS - 1) as f32).round() as usize).min(BINS - 1);
        if let Some(slot) = hist.get_mut(bin) {
            *slot += 1;
            total += 1;
        }
    }
    if total == 0 {
        return (0.0, 0.0);
    }
    let cut = total / 50;
    let mut lo = 0usize;
    let mut acc = 0u64;
    for (i, c) in hist.iter().enumerate() {
        acc += c;
        if acc > cut {
            lo = i;
            break;
        }
    }
    let mut hi = BINS - 1;
    acc = 0;
    for (i, c) in hist.iter().enumerate().rev() {
        acc += c;
        if acc > cut {
            hi = i;
            break;
        }
    }
    (lo as f32 / (BINS - 1) as f32, hi as f32 / (BINS - 1) as f32)
}

/// Gaussian-like blur (three box passes each way) of straight-alpha pixels, done on
/// premultiplied values so transparent pixels do not bleed their colour. Edges are replicated.
fn blur(w: usize, h: usize, px: &mut [[f32; 4]], radius: f32) {
    let r = radius.round().max(0.0) as usize;
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    for p in px.iter_mut() {
        let a = p[3];
        p[0] *= a;
        p[1] *= a;
        p[2] *= a;
    }
    // Three boxes of radius r/2 or so approximate a Gaussian of sigma ≈ r/2 (central limit).
    let box_r = (r / 2).max(1);
    let mut line: Vec<[f32; 4]> = vec![[0.0; 4]; w.max(h)];
    for _ in 0..3 {
        for y in 0..h {
            let Some(row) = px.get_mut(y * w..(y + 1) * w) else { continue };
            box_line(row.iter().copied(), w, box_r, &mut line);
            row.copy_from_slice(line.get(..w).unwrap_or(&[]));
        }
        for x in 0..w {
            box_line((0..h).filter_map(|y| px.get(y * w + x).copied()), h, box_r, &mut line);
            for (y, v) in line.iter().take(h).enumerate() {
                if let Some(slot) = px.get_mut(y * w + x) {
                    *slot = *v;
                }
            }
        }
    }
    for p in px.iter_mut() {
        let a = p[3].clamp(0.0, 1.0);
        if a > 1e-6 {
            p[0] /= a;
            p[1] /= a;
            p[2] /= a;
        } else {
            *p = [0.0; 4];
        }
        p[3] = a;
    }
}

/// One box-filter pass over a line of `len` pixels, edges replicated; the result goes to `out`.
fn box_line(src: impl Iterator<Item = [f32; 4]>, len: usize, r: usize, out: &mut [[f32; 4]]) {
    let line: Vec<[f32; 4]> = src.collect();
    if line.len() != len || len == 0 || out.len() < len {
        return;
    }
    let at = |i: isize| -> [f32; 4] {
        let i = i.clamp(0, len as isize - 1) as usize;
        line.get(i).copied().unwrap_or([0.0; 4])
    };
    let r = r as isize;
    let norm = 1.0 / (2 * r + 1) as f32;
    // f64 running sums: the window slides over up to 4096 pixels and must not drift.
    let mut sum = [0.0f64; 4];
    for i in -r..=r {
        let p = at(i);
        for c in 0..4 {
            sum[c] += f64::from(p[c]);
        }
    }
    for (i, slot) in out.iter_mut().take(len).enumerate() {
        *slot = [sum[0] as f32 * norm, sum[1] as f32 * norm, sum[2] as f32 * norm, sum[3] as f32 * norm];
        let i = i as isize;
        let (add, sub) = (at(i + r + 1), at(i - r));
        for c in 0..4 {
            sum[c] += f64::from(add[c]) - f64::from(sub[c]);
        }
    }
}

/// A frame: a rectangle of the picture in pixels (`x`, `y` its top-left corner).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// How [`frames`] picks its rectangles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FrameParams {
    /// How many frames (1–[`MAX_FRAMES`]).
    pub count: u32,
    /// Smallest and largest frame, as a fraction of the picture's shorter side (0.05–1).
    pub min_fraction: f32,
    pub max_fraction: f32,
    /// Width ÷ height of every frame; `None` keeps the picture's own proportions.
    pub aspect: Option<f32>,
}

impl Default for FrameParams {
    fn default() -> Self {
        Self { count: 6, min_fraction: 0.2, max_fraction: 0.5, aspect: None }
    }
}

/// SplitMix64 (Steele, Lea & Flood 2014): a tiny, well-distributed generator. Everything
/// "chance" in the seeing tools is a pure function of a seed drawn through this, so a result
/// can always be made again from its seed.
#[derive(Clone, Debug)]
pub struct Chance(u64);

impl Chance {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in 0..1 (24 bits, exact in `f32`); never 1.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in `lo..hi`.
    pub fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    /// Uniform index below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { ((self.unit() * n as f32) as usize).min(n - 1) }
    }
}

/// `count` random frames inside a `width` × `height` picture, chosen by `seed` alone: where the
/// "hoop" lands is not up to the viewer's habits. Every frame lies fully inside the picture and
/// is at least one pixel each way.
pub fn frames(seed: u64, width: u32, height: u32, params: FrameParams) -> Result<Vec<Frame>, SeeingError> {
    if width == 0 || height == 0 {
        return Err(SeeingError::BadBuffer);
    }
    if params.count == 0 || params.count as usize > MAX_FRAMES {
        return Err(SeeingError::BadParam("count must be between 1 and 64"));
    }
    let ok = |f: f32| f.is_finite() && (0.05..=1.0).contains(&f);
    if !ok(params.min_fraction) || !ok(params.max_fraction) || params.min_fraction > params.max_fraction {
        return Err(SeeingError::BadParam("minFraction and maxFraction must be between 0.05 and 1, minFraction not above maxFraction"));
    }
    let aspect = match params.aspect {
        None => width as f32 / height as f32,
        Some(a) if a.is_finite() && (0.1..=10.0).contains(&a) => a,
        Some(_) => return Err(SeeingError::BadParam("aspect must be between 0.1 and 10")),
    };
    let short = width.min(height) as f32;
    let mut rng = Chance::new(seed);
    let mut out = Vec::with_capacity(params.count as usize);
    for _ in 0..params.count {
        let f = params.min_fraction + (params.max_fraction - params.min_fraction) * rng.unit();
        // `f` sizes the frame's shorter side; the aspect gives the other, then both fit the picture.
        let side = (short * f).max(1.0);
        let (mut fw, mut fh) = if aspect >= 1.0 { (side * aspect, side) } else { (side, side / aspect) };
        let fit = (width as f32 / fw).min(height as f32 / fh).min(1.0);
        fw *= fit;
        fh *= fit;
        let fw = (fw.round() as u32).clamp(1, width);
        let fh = (fh.round() as u32).clamp(1, height);
        let x = (rng.unit() * (width - fw + 1) as f32) as u32;
        let y = (rng.unit() * (height - fh + 1) as f32) as u32;
        out.push(Frame { x: x.min(width - fw), y: y.min(height - fh), width: fw, height: fh });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64×48 test card: a dark left half, a light right half, a saturated red square on the
    /// dark side and fine one-pixel stripes in the bottom rows.
    fn card() -> (usize, usize, Vec<[f32; 4]>) {
        let (w, h) = (64usize, 48usize);
        let mut px = vec![[0.0f32; 4]; w * h];
        for y in 0..h {
            for x in 0..w {
                let base = if x < w / 2 { 0.15 } else { 0.85 };
                let mut c = [base, base, base, 1.0];
                if (8..20).contains(&x) && (8..20).contains(&y) {
                    c = [0.9, 0.1, 0.1, 1.0];
                }
                if y >= 40 {
                    let s = if x % 2 == 0 { 0.4 } else { 0.6 };
                    c = [s, s, s, 1.0];
                }
                px[y * w + x] = c;
            }
        }
        (w, h, px)
    }

    #[test]
    fn luminance_is_grey_and_keeps_tone_and_alpha() {
        let (w, h, mut px) = card();
        px[0][3] = 0.25;
        let before = px.clone();
        apply_view(View::Luminance, ViewParams::default(), w, h, &mut px).unwrap();
        for (a, b) in px.iter().zip(&before) {
            assert_eq!(a[0], a[1]);
            assert_eq!(a[1], a[2]);
            assert!((a[0] - lum([b[0], b[1], b[2]])).abs() < 1e-6);
            assert_eq!(a[3], b[3]);
        }
    }

    #[test]
    fn masses_removes_fine_detail_but_keeps_big_areas() {
        let (w, h, mut px) = card();
        apply_view(View::Masses, ViewParams { strength: 0.0, ..Default::default() }, w, h, &mut px).unwrap();
        // The one-pixel stripes average out…
        let stripe = (px[44 * w + 10][0] - px[44 * w + 11][0]).abs();
        assert!(stripe < 0.02, "stripes survive: {stripe}");
        // …while the dark and light halves are still far apart.
        assert!(px[30 * w + 60][0] - px[30 * w + 2][0] > 0.4);
    }

    #[test]
    fn shapes_uses_exactly_the_requested_number_of_tones() {
        for levels in [2u32, 3, 5] {
            let (w, h, mut px) = card();
            apply_view(View::Shapes, ViewParams { strength: 0.2, levels }, w, h, &mut px).unwrap();
            let mut tones: Vec<u32> = px.iter().map(|p| (p[0] * 1000.0).round() as u32).collect();
            tones.sort_unstable();
            tones.dedup();
            assert!(tones.len() <= levels as usize, "{levels} levels gave {tones:?}");
            assert!(tones.len() >= 2);
            assert!(tones.contains(&0) && tones.contains(&1000));
        }
    }

    #[test]
    fn shapes_of_a_flat_picture_stays_flat() {
        let mut px = vec![[0.3, 0.3, 0.3, 1.0]; 16 * 16];
        apply_view(View::Shapes, ViewParams::default(), 16, 16, &mut px).unwrap();
        assert!(px.iter().all(|p| (p[0] - 0.3).abs() < 1e-4));
    }

    #[test]
    fn lines_marks_the_big_edge_and_leaves_flat_areas_white() {
        let (w, h, mut px) = card();
        apply_view(View::Lines, ViewParams { strength: 0.0, ..Default::default() }, w, h, &mut px).unwrap();
        // On the dark/light boundary: ink. In the middle of the light half: paper.
        assert!(px[30 * w + 32][0] < 0.5, "no line on the edge: {}", px[30 * w + 32][0]);
        assert!(px[30 * w + 55][0] > 0.95);
        assert!(px.iter().all(|p| p[3] == 1.0));
    }

    #[test]
    fn lines_leaves_fine_texture_as_paper() {
        // A strong edge down the middle, with faint grain (±2 %) everywhere.
        let (w, h) = (96usize, 64usize);
        let mut rng = Chance::new(3);
        let mut px: Vec<[f32; 4]> = (0..w * h)
            .map(|i| {
                let g = if i % w < w / 2 { 0.2 } else { 0.8 } + rng.between(-0.02, 0.02);
                [g, g, g, 1.0]
            })
            .collect();
        apply_view(View::Lines, ViewParams::default(), w, h, &mut px).unwrap();
        assert!(px[32 * w + 48][0] < 0.5, "the edge is not drawn: {}", px[32 * w + 48][0]);
        // Away from the edge the grain leaves no ink at all.
        for y in 4..h - 4 {
            for x in (4..w / 2 - 8).chain(w / 2 + 8..w - 4) {
                assert!(px[y * w + x][0] > 0.99, "grain drawn at {x},{y}: {}", px[y * w + x][0]);
            }
        }
    }

    #[test]
    fn lines_of_an_even_pattern_does_not_go_black() {
        // Stripes everywhere, all equally strong: every pixel is near an "edge".
        let (w, h) = (128usize, 96usize);
        let mut px: Vec<[f32; 4]> = (0..w * h)
            .map(|i| {
                let g = 0.5 + 0.3 * (((i % w) + (i / w)) as f32 / 6.0).sin();
                [g, g, g, 1.0]
            })
            .collect();
        apply_view(View::Lines, ViewParams { strength: 0.0, ..Default::default() }, w, h, &mut px).unwrap();
        let inked = px.iter().filter(|p| p[0] < 0.5).count() as f32 / px.len() as f32;
        assert!(inked < 0.25, "{:.0} % of an evenly striped picture is ink", inked * 100.0);
        assert!(inked > 0.02, "the stripes left no lines at all");
        // A transparent picture's lines are still drawn on opaque paper.
        let mut px: Vec<[f32; 4]> = (0..w * h).map(|i| if i % w < w / 2 { [0.0; 4] } else { [0.2, 0.2, 0.2, 1.0] }).collect();
        apply_view(View::Lines, ViewParams::default(), w, h, &mut px).unwrap();
        assert!(px.iter().all(|p| p[3] == 1.0));
        assert!(px[40 * w + w / 2][0] < 0.5, "the shape's edge against transparency is a line");
    }

    #[test]
    fn lines_of_a_flat_picture_is_blank_paper() {
        let mut px = vec![[0.5, 0.2, 0.7, 1.0]; 20 * 20];
        apply_view(View::Lines, ViewParams::default(), 20, 20, &mut px).unwrap();
        assert!(px.iter().all(|p| p[0] == 1.0));
    }

    #[test]
    fn color_view_equalises_tone_and_keeps_hue() {
        let (w, h, mut px) = card();
        apply_view(View::Color, ViewParams::default(), w, h, &mut px).unwrap();
        // The dark and light greys become the same grey…
        assert!((px[30 * w + 2][0] - px[30 * w + 60][0]).abs() < 1e-4);
        // …and the red square is still red.
        let red = px[12 * w + 12];
        assert!(red[0] > red[1] + 0.2 && red[0] > red[2] + 0.2, "{red:?}");
    }

    #[test]
    fn views_are_deterministic() {
        for view in View::ALL {
            let (w, h, mut a) = card();
            let mut b = a.clone();
            apply_view(view, ViewParams::default(), w, h, &mut a).unwrap();
            apply_view(view, ViewParams::default(), w, h, &mut b).unwrap();
            assert_eq!(a, b, "{view:?}");
        }
    }

    #[test]
    fn blur_does_not_bleed_colour_out_of_transparency() {
        // A transparent green left half next to an opaque red right half.
        let (w, h) = (32usize, 8usize);
        let mut px: Vec<[f32; 4]> = (0..w * h).map(|i| if i % w < w / 2 { [0.0, 1.0, 0.0, 0.0] } else { [1.0, 0.0, 0.0, 1.0] }).collect();
        apply_view(View::Masses, ViewParams::default(), w, h, &mut px).unwrap();
        for p in &px {
            assert!(p[1] < 1e-3, "green bled in: {p:?}");
        }
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let mut px = vec![[0.0; 4]; 10];
        assert_eq!(apply_view(View::Masses, ViewParams::default(), 0, 5, &mut px), Err(SeeingError::BadBuffer));
        assert_eq!(apply_view(View::Masses, ViewParams::default(), 3, 5, &mut px), Err(SeeingError::BadBuffer));
        assert_eq!(apply_view(View::Masses, ViewParams::default(), usize::MAX, 2, &mut px), Err(SeeingError::BadBuffer));
        assert!(matches!(apply_view(View::Shapes, ViewParams { levels: 1, ..Default::default() }, 2, 5, &mut px), Err(SeeingError::BadParam(_))));
        assert!(matches!(apply_view(View::Shapes, ViewParams { strength: f32::NAN, ..Default::default() }, 2, 5, &mut px), Err(SeeingError::BadParam(_))));
        // Non-finite samples do not spread.
        let mut px = vec![[f32::NAN, f32::INFINITY, 0.5, 1.0]; 16];
        apply_view(View::Masses, ViewParams::default(), 4, 4, &mut px).unwrap();
        assert!(px.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn frames_repeat_for_a_seed_and_differ_between_seeds() {
        let p = FrameParams::default();
        let a = frames(7, 6000, 4000, p).unwrap();
        assert_eq!(a, frames(7, 6000, 4000, p).unwrap());
        assert_ne!(a, frames(8, 6000, 4000, p).unwrap());
        assert_eq!(a.len(), 6);
    }

    #[test]
    fn frames_stay_inside_and_keep_the_aspect() {
        for (w, h) in [(6000u32, 4000u32), (400, 3000), (1, 1), (3, 1000)] {
            for aspect in [None, Some(1.0), Some(16.0 / 9.0), Some(0.5)] {
                let p = FrameParams { count: 32, min_fraction: 0.05, max_fraction: 1.0, aspect };
                for f in frames(99, w, h, p).unwrap() {
                    assert!(f.width >= 1 && f.height >= 1);
                    assert!(u64::from(f.x) + u64::from(f.width) <= u64::from(w), "{f:?} in {w}x{h}");
                    assert!(u64::from(f.y) + u64::from(f.height) <= u64::from(h), "{f:?} in {w}x{h}");
                    if let Some(a) = aspect
                        && f.width > 20
                        && f.height > 20
                    {
                        let got = f.width as f32 / f.height as f32;
                        assert!((got / a - 1.0).abs() < 0.1, "aspect {got} wanted {a}");
                    }
                }
            }
        }
    }

    #[test]
    fn frame_params_are_validated() {
        let p = FrameParams::default();
        assert!(frames(1, 0, 10, p).is_err());
        assert!(frames(1, 10, 10, FrameParams { count: 0, ..p }).is_err());
        assert!(frames(1, 10, 10, FrameParams { count: 65, ..p }).is_err());
        assert!(frames(1, 10, 10, FrameParams { min_fraction: 0.9, max_fraction: 0.2, ..p }).is_err());
        assert!(frames(1, 10, 10, FrameParams { min_fraction: f32::NAN, ..p }).is_err());
        assert!(frames(1, 10, 10, FrameParams { aspect: Some(0.0), ..p }).is_err());
        assert!(frames(1, 10, 10, FrameParams { aspect: Some(f32::INFINITY), ..p }).is_err());
    }
}
