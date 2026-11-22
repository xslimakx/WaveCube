//! Small DSP building blocks: state-variable filters, band splits, random numbers, curves.

use std::f32::consts::{LN_10, PI};

#[inline]
pub fn db2lin(db: f32) -> f32 {
    (db * (LN_10 / 20.0)).exp()
}

/// Fast log2 (error < 0.001): level detectors run it every sample.
#[inline]
pub fn fast_log2(x: f32) -> f32 {
    let x = x.max(1e-12);
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xff) as f32 - 127.0;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000); // 1 .. 2
    // fitted polynomial for log2(m) on [1, 2), exact at 1
    let p = (m - 1.0) * (2.524_500_6 + (-1.578_112 + (0.576_430 - 0.084_273_16 * m) * m) * m);
    e + p
}

/// Fast 2^x (relative error < 2e-4).
#[inline]
pub fn fast_exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let i = x.floor();
    let f = x - i;
    let p = 1.0 + f * (0.695_430 + f * (0.226_940_1 + f * 0.077_380_64));
    f32::from_bits(((i as i32 + 127) as u32) << 23) * p
}

/// Fast level in dB.
#[inline]
pub fn fast_db(x: f32) -> f32 {
    6.020_6 * fast_log2(x)
}

/// Fast dB to gain.
#[inline]
pub fn fast_db2lin(db: f32) -> f32 {
    fast_exp2(db * (1.0 / 6.020_6))
}

/// one-pole smoothing coefficient for a time constant in ms
#[inline]
pub fn coef_ms(ms: f32, sr: f32) -> f32 {
    if ms < 0.005 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

/// one-pole low-pass coefficient for a cutoff in Hz
#[inline]
pub fn coef_hz(hz: f32, sr: f32) -> f32 {
    1.0 - (-2.0 * PI * hz.clamp(1.0, sr * 0.45) / sr).exp()
}

/// Trapezoidal state-variable filter (Cytomic / Simper).
#[derive(Clone, Copy, Default, Debug)]
pub struct Svf {
    pub ic1: f32,
    pub ic2: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct SvfCoef {
    pub g: f32,
    pub k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoef {
    pub fn new(f: f32, sr: f32, k: f32) -> Self {
        let g = (PI * (f / sr).clamp(1e-5, 0.49)).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { g, k, a1, a2, a3: g * a2 }
    }
}

impl Svf {
    /// (low, band, high)
    #[inline]
    pub fn run(&mut self, c: &SvfCoef, v0: f32) -> (f32, f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, v0 - c.k * v1 - v2)
    }

    /// one-pole (6 dB) using the same g; returns (low, high)
    #[inline]
    pub fn one_pole(&mut self, c: &SvfCoef, x: f32) -> (f32, f32) {
        let v = (x - self.ic1) * c.g / (1.0 + c.g);
        let lp = v + self.ic1;
        self.ic1 = lp + v;
        (lp, x - lp)
    }

    /// keeps the integrators bounded (warm filters at high resonance)
    #[inline]
    pub fn saturate(&mut self, limit: f32) {
        self.ic1 = limit * (self.ic1 / limit).tanh();
        self.ic2 = limit * (self.ic2 / limit).tanh();
    }
}

/// A complementary low / high split at 6, 12 or 24 dB/oct whose two outputs sum to a flat
/// (all-pass) response.
#[derive(Clone, Copy, Default)]
pub struct Split2 {
    a: Svf,
    lp: Svf,
    hp: Svf,
}

impl Split2 {
    /// slope: 0 = 6 dB, 1 = 12 dB, 2 = 24 dB (Linkwitz-Riley)
    #[inline]
    pub fn run(&mut self, c: &SvfCoef, slope: u8, x: f32) -> (f32, f32) {
        match slope {
            0 => {
                let (l, h) = self.a.one_pole(c, x);
                (l, h)
            }
            1 => {
                // LR2 = two first-order... use the 2nd-order Butterworth pair with the high
                // band inverted: sums to an all-pass
                let (l, _, h) = self.a.run(c, x);
                (l, -h)
            }
            _ => {
                let (l, _, h) = self.a.run(c, x);
                let (ll, _, _) = self.lp.run(c, l);
                let (_, _, hh) = self.hp.run(c, h);
                (ll, hh)
            }
        }
    }
}

/// Three bands; the low band also goes through the all-pass of the upper split so all three
/// stay in phase and sum flat.
#[derive(Clone, Copy, Default)]
pub struct Split3 {
    x1: Split2,
    x2: Split2,
    ap: Split2,
}

impl Split3 {
    #[inline]
    pub fn run(&mut self, c1: &SvfCoef, c2: &SvfCoef, slope: u8, x: f32) -> [f32; 3] {
        let (lo, rest) = self.x1.run(c1, slope, x);
        let (mid, hi) = self.x2.run(c2, slope, rest);
        let (a, b) = self.ap.run(c2, slope, lo);
        [a + b, mid, hi]
    }
}

/// The coefficient a split of this slope needs (Butterworth damping for 12 / 24 dB; for the
/// 12 dB pair the damping that makes LP - HP flat: k = 2, i.e. two real poles).
pub fn split_coef(f: f32, sr: f32, slope: u8) -> SvfCoef {
    match slope {
        1 => SvfCoef::new(f, sr, 2.0),
        _ => SvfCoef::new(f, sr, std::f32::consts::SQRT_2),
    }
}

pub struct Rng(pub u32);

impl Rng {
    /// -1 .. 1
    #[inline]
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
    /// 0 .. 1
    #[inline]
    pub fn uni(&mut self) -> f32 {
        0.5 * (self.next() + 1.0)
    }
}

#[inline]
pub fn fold(v: f32) -> f32 {
    1.0 - ((v + 1.0).rem_euclid(4.0) - 2.0).abs()
}

/// The 12 drive curves (kinds::DRIVE_STYLES).
#[inline]
pub fn drive_curve(style: u8, v: f32) -> f32 {
    match style {
        0 => v.tanh(),
        1 => {
            // smooth clip: cubic soft knee
            let a = v.clamp(-1.5, 1.5);
            a - a * a * a / 6.75
        }
        2 => v.clamp(-1.0, 1.0),
        3 => (v * 4.0).tanh(),
        4 => (v * 40.0).clamp(-1.0, 1.0),
        5 => (v.abs() * 1.5).tanh() * 1.2 - 0.3,
        6 => v.abs().min(1.0) * 1.4 - 0.4,
        7 => fold(v + 0.4 * v * v.abs()),
        8 => (v * PI * 0.5).sin(),
        9 => fold(v),
        10 => {
            // saw fold: wraps around
            ((v + 1.0).rem_euclid(2.0)) - 1.0
        }
        _ => fold(v * 2.5 + (v * 5.0).sin() * 0.6),
    }
}

/// Soft safety clipper: transparent below 0.9, never above 1.0.
#[inline]
pub fn safe(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 {
        x
    } else {
        (0.9 + 0.1 * ((a - 0.9) * 10.0).tanh()).copysign(x)
    }
}

/// Swing: the second step of every pair of grid steps starts later.
#[inline]
pub fn swing_warp(ph: f32, swing: f32, grid: f32) -> f32 {
    if swing < 1e-4 {
        return ph;
    }
    let pair = 2.0 / grid;
    let base = (ph / pair).floor() * pair;
    let t = (ph - base) / pair;
    let mid = 0.5 + 0.25 * swing;
    let t2 = if t < mid { 0.5 * t / mid } else { 0.5 + 0.5 * (t - mid) / (1.0 - mid) };
    (base + t2 * pair).min(0.999_999)
}

/// Linear-interpolated read from a ring buffer, `d` samples behind `w` (the newest sample).
#[inline]
pub fn ring_read(buf: &[[f32; 2]], w: usize, d: f32, ch: usize) -> f32 {
    let n = buf.len();
    let pos = w as f32 - d;
    let i = pos.floor();
    let t = pos - i;
    let i0 = (i as isize).rem_euclid(n as isize) as usize;
    let i1 = if d < 1.0 { w } else { (i0 + 1) % n };
    buf[i0][ch] + (buf[i1][ch] - buf[i0][ch]) * t
}

/// Magnitude (dB) of a filter at `f` for the response display: kind of filter, slope, cutoff,
/// damping k.
pub fn filter_mag_db(f_type: u8, slope: u8, fc: f32, k: f32, f: f32) -> f32 {
    // analog prototype, s = j w / wc
    let w = f / fc.max(1.0);
    let (re, im) = (0.0f32, w);
    let s2 = (re * re - im * im, 2.0 * re * im);
    // denominator s^2 + k s + 1
    let den = (s2.0 + k * re + 1.0, s2.1 + k * im);
    let den_m = (den.0 * den.0 + den.1 * den.1).sqrt().max(1e-9);
    let num = match f_type {
        0 => 1.0,
        1 => (s2.0 * s2.0 + s2.1 * s2.1).sqrt(),
        2 => k * (re * re + im * im).sqrt(),
        3 => ((s2.0 + 1.0).powi(2) + s2.1 * s2.1).sqrt(),
        _ => {
            // peak: (1 - s^2) / den ... |LP - HP|
            ((1.0 - s2.0).powi(2) + s2.1 * s2.1).sqrt()
        }
    };
    let one = match (slope, f_type) {
        (0, 0) => 1.0 / (1.0 + w * w).sqrt(),
        (0, 1) => w / (1.0 + w * w).sqrt(),
        _ => num / den_m,
    };
    let mag = if slope == 2 { one * one } else { one };
    20.0 * mag.max(1e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sweep_flat(slope: u8) {
        let sr = 48000.0;
        let c1 = split_coef(200.0, sr, slope);
        let c2 = split_coef(2500.0, sr, slope);
        for f in [50.0f32, 200.0, 800.0, 2500.0, 9000.0] {
            let mut s = Split3::default();
            let mut pin = 0.0;
            let mut pout = 0.0;
            for i in 0..48000 {
                let x = (2.0 * PI * f * i as f32 / sr).sin();
                let b = s.run(&c1, &c2, slope, x);
                let y = b[0] + b[1] + b[2];
                if i > 24000 {
                    pin += x * x;
                    pout += y * y;
                }
            }
            let db = 10.0 * (pout / pin).log10();
            assert!(db.abs() < 0.15, "slope {slope} {f} Hz: {db} dB");
        }
    }

    #[test]
    fn splits_sum_flat_at_every_slope() {
        for s in 0..3 {
            sweep_flat(s);
        }
    }

    #[test]
    fn fast_math_is_close() {
        for i in 1..2000 {
            let x = i as f32 * 0.37e-3 * (i as f32).sqrt();
            assert!((fast_log2(x) - x.log2()).abs() < 2e-3, "{x}");
        }
        for i in -400..400 {
            let x = i as f32 * 0.05;
            let r = fast_exp2(x) / x.exp2();
            assert!((r - 1.0).abs() < 5e-4, "{x} {r}");
        }
    }

    #[test]
    fn drive_curves_are_bounded() {
        for s in 0..12u8 {
            for i in -400..=400 {
                let v = drive_curve(s, i as f32 / 40.0);
                assert!(v.is_finite() && v.abs() <= 1.5, "{s} {v}");
            }
        }
    }
}
