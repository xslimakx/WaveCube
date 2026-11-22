//! Reverb: an 8-line feedback delay network with input diffusion, early reflections,
//! modulation, shimmer and gating for the built-in spaces, and a partitioned FFT convolution for
//! user impulse responses.

use super::blocks::coef_hz;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

const LINES: usize = 8;
const MAX_LINE_S: f32 = 0.32;
const PRE_S: f32 = 0.52;
const SHIMMER_S: f32 = 0.1;

#[derive(Clone, Copy)]
struct Space {
    lens_ms: [f32; LINES],
    rt: f32,
    damp: f32,
    diffusion: f32,
    mod_depth: f32,
    mod_hz: f32,
    early: bool,
    shimmer: bool,
    gated: bool,
}

const HALL: [f32; LINES] = [43.1, 51.7, 61.3, 67.9, 79.2, 87.6, 97.1, 113.0];
const PLATE: [f32; LINES] = [11.0, 13.7, 17.3, 19.1, 23.9, 27.1, 31.3, 37.1];

fn scale(l: [f32; LINES], k: f32) -> [f32; LINES] {
    l.map(|v| v * k)
}

/// kinds::REVERBS
fn space(t: usize) -> Space {
    let s = |lens_ms, rt, damp, diffusion, mod_depth, mod_hz, early, shimmer, gated| Space { lens_ms, rt, damp, diffusion, mod_depth, mod_hz, early, shimmer, gated };
    match t {
        0 => s(HALL, 2.6, 7000.0, 0.7, 3.0, 0.6, true, false, false),
        1 => s(HALL, 2.2, 13000.0, 0.7, 3.0, 0.7, true, false, false),
        2 => s(scale(HALL, 0.33), 0.5, 6000.0, 0.6, 1.0, 0.5, true, false, false),
        3 => s(scale(HALL, 0.5), 0.8, 9000.0, 0.65, 1.5, 0.5, true, false, false),
        4 => s(scale(HALL, 0.6), 1.3, 8000.0, 0.7, 2.0, 0.4, true, false, false),
        5 => s(PLATE, 1.8, 11000.0, 0.75, 2.0, 0.8, false, false, false),
        6 => s(PLATE, 1.3, 8000.0, 0.75, 2.0, 0.8, false, false, false),
        7 => s(scale(HALL, 1.4), 5.0, 5000.0, 0.7, 4.0, 0.3, true, false, false),
        8 => s(scale(HALL, 1.2), 30.0, 6000.0, 0.7, 6.0, 0.2, false, false, false),
        9 => s(HALL, 4.0, 9000.0, 0.7, 4.0, 0.3, false, true, false),
        10 => s(PLATE, 2.5, 9000.0, 0.7, 20.0, 0.9, false, false, false),
        11 => s([3.1, 4.1, 5.3, 6.7, 7.9, 9.7, 11.3, 13.1], 1.5, 4500.0, 0.75, 1.0, 1.7, false, false, false),
        12 => s([1.3, 1.7, 2.1, 2.3, 2.7, 2.9, 3.1, 3.7], 2.0, 9000.0, 0.5, 0.5, 0.3, false, false, false),
        _ => s(scale(HALL, 0.6), 1.0, 9000.0, 0.7, 2.0, 0.5, true, false, true),
    }
}

#[derive(Clone, Copy, Default)]
struct Allpass {
    idx: usize,
    len: usize,
}

/// `i % n` for `i < 2 n` (no division)
#[inline(always)]
fn wrap(i: usize, n: usize) -> usize {
    if i >= n {
        i - n
    } else {
        i
    }
}

/// sin(2 pi t), t in turns (parabolic approximation, plenty for modulation and fades)
#[inline]
fn fsin(t: f32) -> f32 {
    let x = t - t.floor();
    let y = if x < 0.5 { 8.0 * x * (1.0 - 2.0 * x) } else { -8.0 * (x - 0.5) * (1.0 - 2.0 * (x - 0.5)) };
    // one refinement step: max error ~0.1 %
    0.225 * (y * y.abs() - y) + y
}

pub struct Fdn {
    sr: f32,
    lines: Vec<Vec<f32>>,
    w: usize,
    len: [f32; LINES],
    lp: [f32; LINES],
    pre: Vec<[f32; 2]>,
    pw: usize,
    ap_buf: Vec<Vec<f32>>,
    ap: [Allpass; 8],
    shim: Vec<f32>,
    sw: usize,
    sph: f32,
    mph: f32,
    gate: f32,
    gate_hold: u32,
    env: f32,
    /// cached loop gains: (rt, damp, space) -> g per line, damping coefficient
    gk: (f32, f32, usize, [f32; LINES], f32),
    gk_n: u32,
    /// the space and the tail time, cached
    sp: (usize, Space),
    rt: f32,
}

const AP_MS: [f32; 4] = [4.77, 3.59, 12.73, 9.31];

impl Fdn {
    pub fn new(sr: f32) -> Self {
        let n = (MAX_LINE_S * sr) as usize + 8;
        let ap_buf = (0..8).map(|i| vec![0.0; (AP_MS[i % 4] * 0.001 * sr * 2.2) as usize + 4]).collect();
        Self {
            sr,
            lines: (0..LINES).map(|_| vec![0.0; n]).collect(),
            w: 0,
            len: [0.03 * sr; LINES],
            lp: [0.0; LINES],
            pre: vec![[0.0; 2]; (PRE_S * sr) as usize + 4],
            pw: 0,
            ap_buf,
            ap: [Allpass::default(); 8],
            shim: vec![0.0; (SHIMMER_S * sr) as usize + 4],
            sw: 0,
            sph: 0.0,
            mph: 0.0,
            gate: 1.0,
            gate_hold: 0,
            env: 0.0,
            gk: (-1.0, -1.0, usize::MAX, [0.0; LINES], 1.0),
            gk_n: 0,
            sp: (0, space(0)),
            rt: 1.0,
        }
    }

    pub fn clear(&mut self) {
        for l in self.lines.iter_mut() {
            l.iter_mut().for_each(|v| *v = 0.0);
        }
        for b in self.ap_buf.iter_mut() {
            b.iter_mut().for_each(|v| *v = 0.0);
        }
        self.shim.iter_mut().for_each(|v| *v = 0.0);
        self.lp = [0.0; LINES];
    }

    /// One stereo sample. Returns the wet signal.
    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, x: [f32; 2], t: usize, pre_ms: f32, decay: f32, size: f32, width: f32, tone: f32, gated: bool) -> [f32; 2] {
        if self.sp.0 != t {
            self.sp = (t, space(t));
        }
        let sp = self.sp.1;
        let sr = self.sr;
        let n = self.lines[0].len();
        // pre-delay
        let pn = self.pre.len();
        self.pre[self.pw] = x;
        let pd = (pre_ms * 0.001 * sr).clamp(0.0, (pn - 2) as f32) as usize;
        let xi = self.pre[wrap(self.pw + pn - pd, pn)];
        self.pw = wrap(self.pw + 1, pn);
        // input diffusion: 4 all-passes per channel
        let mut inp = xi;
        for (ch, v) in inp.iter_mut().enumerate() {
            for k in 0..4 {
                let i = ch * 4 + k;
                let len = ((AP_MS[k] * 0.001 * sr * size.min(2.0)) as usize).clamp(2, self.ap_buf[i].len() - 1);
                let b = &mut self.ap_buf[i];
                let a = &mut self.ap[i];
                a.len = len;
                let rd = b[wrap(a.idx + b.len() - len, b.len())];
                let g = sp.diffusion;
                let wv = *v + g * rd;
                b[a.idx] = wv;
                *v = rd - g * wv;
                a.idx = wrap(a.idx + 1, b.len());
            }
        }
        // shimmer: an octave up of the tail, fed back in
        let mut shimmer = 0.0;
        if sp.shimmer {
            let sn = self.shim.len();
            let win = sn as f32 - 2.0;
            self.sph = (self.sph + 1.0 / win) % 1.0;
            let read = |ph: f32, buf: &Vec<f32>, w: usize| {
                let d = (1.0 - ph) * win;
                let pos = w as f32 - d;
                let i = pos.floor();
                let f = pos - i;
                let i0 = (i as isize).rem_euclid(sn as isize) as usize;
                buf[i0] * (1.0 - f) + buf[(i0 + 1) % sn] * f
            };
            let a = read(self.sph, &self.shim, self.sw);
            let b = read((self.sph + 0.5) % 1.0, &self.shim, self.sw);
            let wa = fsin(0.5 * self.sph);
            let wb = fsin(0.5 * ((self.sph + 0.5) % 1.0));
            shimmer = (a * wa + b * wb) * 0.7;
        }
        // the network
        // loop gains: recomputed every 32 samples or when the settings move
        self.gk_n = self.gk_n.wrapping_add(1);
        if self.gk.2 != t || (self.gk.0 - decay).abs() > 1e-4 || (self.gk.1 - tone).abs() > 1e-4 || self.gk_n % 32 == 0 {
            let rt = (sp.rt * 2f32.powf((decay - 0.55) * 4.0)).max(0.05);
            self.rt = rt;
            let mut g = [0.0; LINES];
            for (i, gi) in g.iter_mut().enumerate() {
                *gi = 10f32.powf(-3.0 * self.len[i] / (rt * sr)).min(0.9995);
            }
            self.gk = (decay, tone, t, g, coef_hz(sp.damp * 2f32.powf((tone - 0.55) * 3.0), sr));
        }
        let rt = self.rt;
        let damp = self.gk.4;
        self.mph = (self.mph + sp.mod_hz / sr) % 1.0;
        let mut o = [0.0f32; LINES];
        for i in 0..LINES {
            let target = (sp.lens_ms[i] * 0.001 * sr * size).clamp(4.0, (n - 4) as f32);
            self.len[i] += (target - self.len[i]) * 0.0005;
            let m = if i % 2 == 1 { sp.mod_depth * fsin(self.mph + i as f32 * 0.13) } else { 0.0 };
            let d = (self.len[i] + m).clamp(2.0, (n - 3) as f32);
            let pos = self.w as f32 - d;
            let ii = pos.floor();
            let f = pos - ii;
            let i0 = if ii < 0.0 { (ii as isize + n as isize) as usize } else { ii as usize };
            let i0 = wrap(i0, n);
            let line = &self.lines[i];
            o[i] = line[i0] * (1.0 - f) + line[wrap(i0 + 1, n)] * f;
        }
        let mut v = [0.0f32; LINES];
        let mut sum = 0.0;
        for i in 0..LINES {
            self.lp[i] += (o[i] - self.lp[i]) * damp;
            v[i] = self.lp[i] * self.gk.3[i];
            sum += v[i];
        }
        let h = sum * 2.0 / LINES as f32;
        for i in 0..LINES {
            let feed = if i % 2 == 0 { inp[0] } else { inp[1] };
            let val = feed * 0.35 + v[i] - h + shimmer * 0.25;
            self.lines[i][self.w] = if val.is_finite() { val } else { 0.0 };
        }
        self.w = wrap(self.w + 1, n);
        let mut l = (o[0] - o[2] + o[4] - o[6]) * 0.35;
        let mut r = (o[1] - o[3] + o[5] - o[7]) * 0.35;
        if sp.shimmer {
            let sn = self.shim.len();
            self.shim[self.sw] = (l + r) * 0.5;
            self.sw = (self.sw + 1) % sn;
        }
        // early reflections from the pre-delayed input
        if sp.early {
            const TAPS: [(f32, f32); 6] = [(7.1, 0.55), (11.3, 0.45), (17.9, 0.38), (23.3, 0.3), (31.7, 0.24), (41.9, 0.18)];
            for (k, (ms, g)) in TAPS.iter().enumerate() {
                let d = ((ms * size * 0.001 * sr) as usize).min(pn - 2) + pd;
                let s = self.pre[(self.pw + pn * 2 - 1 - d.min(pn - 1)) % pn];
                if k % 2 == 0 {
                    l += s[0] * g * 0.6;
                } else {
                    r += s[1] * g * 0.6;
                }
            }
        }
        // gate: cut the tail when the input stops
        if gated || sp.gated {
            let lvl = x[0].abs().max(x[1].abs());
            self.env = if lvl > self.env { lvl } else { self.env * 0.9995 };
            if self.env > 0.01 {
                self.gate_hold = (rt * 0.35 * sr) as u32;
            }
            let target = if self.gate_hold > 0 {
                self.gate_hold -= 1;
                1.0
            } else {
                0.0
            };
            self.gate += (target - self.gate) * if target > self.gate { 0.05 } else { 0.003 };
            l *= self.gate;
            r *= self.gate;
        }
        // width
        let m = 0.5 * (l + r);
        let s = 0.5 * (l - r) * width;
        [m + s, m - s]
    }
}

// ------------------------------------------------------------------------------------------
// Convolution (user impulse responses)
// ------------------------------------------------------------------------------------------

pub const CONV_B: usize = 256;

/// Uniformly partitioned FFT convolution, stereo. The output comes CONV_B samples late (on the
/// wet path only, so the plugin has no latency).
pub struct Conv {
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// IR spectra per channel and partition
    h: [Vec<Vec<Complex32>>; 2],
    /// input spectra history per channel (ring)
    x: [Vec<Vec<Complex32>>; 2],
    xi: usize,
    inbuf: [Vec<f32>; 2],
    outbuf: [Vec<f32>; 2],
    pos: usize,
    tmp: Vec<f32>,
    spec: Vec<Complex32>,
    acc: Vec<Complex32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
}

impl Conv {
    /// `ir`: stereo impulse response at the engine rate (built on a non-audio thread).
    pub fn new(ir: &[[f32; 2]]) -> Self {
        let n = 2 * CONV_B;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let parts = ir.len().div_ceil(CONV_B).max(1);
        let bins = n / 2 + 1;
        // normalise the IR loudness a little (long IRs are loud)
        let energy: f32 = ir.iter().map(|s| s[0] * s[0] + s[1] * s[1]).sum::<f32>().max(1e-9);
        let norm = (1.0 / energy.sqrt()).min(4.0);
        let mut h: [Vec<Vec<Complex32>>; 2] = [vec![], vec![]];
        let mut scratch = fwd.make_scratch_vec();
        for (ch, hc) in h.iter_mut().enumerate() {
            for p in 0..parts {
                let mut buf = vec![0.0f32; n];
                for i in 0..CONV_B {
                    if let Some(s) = ir.get(p * CONV_B + i) {
                        buf[i] = s[ch] * norm;
                    }
                }
                let mut out = vec![Complex32::new(0.0, 0.0); bins];
                let _ = fwd.process_with_scratch(&mut buf, &mut out, &mut scratch);
                hc.push(out);
            }
        }
        let x = [vec![vec![Complex32::new(0.0, 0.0); bins]; parts], vec![vec![Complex32::new(0.0, 0.0); bins]; parts]];
        Self {
            scratch_f: fwd.make_scratch_vec(),
            scratch_i: inv.make_scratch_vec(),
            fwd,
            inv,
            h,
            x,
            xi: 0,
            inbuf: [vec![0.0; n], vec![0.0; n]],
            outbuf: [vec![0.0; CONV_B], vec![0.0; CONV_B]],
            pos: 0,
            tmp: vec![0.0; n],
            spec: vec![Complex32::new(0.0, 0.0); bins],
            acc: vec![Complex32::new(0.0, 0.0); bins],
        }
    }

    pub fn clear(&mut self) {
        for c in 0..2 {
            self.inbuf[c].iter_mut().for_each(|v| *v = 0.0);
            self.outbuf[c].iter_mut().for_each(|v| *v = 0.0);
            for s in self.x[c].iter_mut() {
                s.iter_mut().for_each(|v| *v = Complex32::new(0.0, 0.0));
            }
        }
    }

    #[inline]
    pub fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        let out = [self.outbuf[0][self.pos], self.outbuf[1][self.pos]];
        self.inbuf[0][CONV_B + self.pos] = x[0];
        self.inbuf[1][CONV_B + self.pos] = x[1];
        self.pos += 1;
        if self.pos == CONV_B {
            self.pos = 0;
            self.block();
        }
        out
    }

    fn block(&mut self) {
        let n = 2 * CONV_B;
        let parts = self.h[0].len();
        self.xi = (self.xi + parts - 1) % parts;
        for ch in 0..2 {
            self.tmp.copy_from_slice(&self.inbuf[ch]);
            let _ = self.fwd.process_with_scratch(&mut self.tmp, &mut self.spec, &mut self.scratch_f);
            self.x[ch][self.xi].copy_from_slice(&self.spec);
            self.acc.iter_mut().for_each(|v| *v = Complex32::new(0.0, 0.0));
            for p in 0..parts {
                let xs = &self.x[ch][(self.xi + p) % parts];
                let hs = &self.h[ch][p];
                for ((a, xv), hv) in self.acc.iter_mut().zip(xs.iter()).zip(hs.iter()) {
                    *a += xv * hv;
                }
            }
            // real inverse needs a real DC / Nyquist
            self.acc[0].im = 0.0;
            let last = self.acc.len() - 1;
            self.acc[last].im = 0.0;
            let _ = self.inv.process_with_scratch(&mut self.acc, &mut self.tmp, &mut self.scratch_i);
            let scale = 1.0 / n as f32;
            for i in 0..CONV_B {
                self.outbuf[ch][i] = self.tmp[CONV_B + i] * scale;
            }
            // slide the input window
            self.inbuf[ch].copy_within(CONV_B..n, 0);
        }
    }
}

/// One reverb (per band).
pub struct Reverb {
    pub fdn: Fdn,
    pub conv: Option<Conv>,
}

impl Reverb {
    pub fn new(sr: f32, ir: Option<&[[f32; 2]]>) -> Self {
        Self { fdn: Fdn::new(sr), conv: ir.map(Conv::new) }
    }
    pub fn clear(&mut self) {
        self.fdn.clear();
        if let Some(c) = self.conv.as_mut() {
            c.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_space_rings_and_decays() {
        let sr = 48000.0;
        for t in 0..crate::kinds::REVERBS.len() {
            let mut f = Fdn::new(sr);
            let mut e_early = 0.0;
            let mut e_late = 0.0;
            for i in 0..(sr as usize * 3) {
                let x = if i == 0 { [1.0, 1.0] } else { [0.0, 0.0] };
                let y = f.process(x, t, 10.0, 0.4, 1.0, 1.0, 0.5, false);
                assert!(y[0].is_finite() && y[0].abs() < 10.0, "{t}");
                if i < sr as usize / 2 {
                    e_early += y[0] * y[0] + y[1] * y[1];
                } else if i > sr as usize * 2 {
                    e_late += y[0] * y[0] + y[1] * y[1];
                }
            }
            assert!(e_early > 1e-4, "{} silent", crate::kinds::REVERBS[t].1);
            if t != 8 && t != 9 {
                assert!(e_late < e_early, "{} does not decay", crate::kinds::REVERBS[t].1);
            }
        }
    }

    #[test]
    fn convolution_matches_direct() {
        // a short IR: the output must be the input convolved, CONV_B samples late
        let ir: Vec<[f32; 2]> = (0..700).map(|i| [((i as f32) * 0.37).sin() * (-(i as f32) / 200.0).exp(), if i == 3 { 1.0 } else { 0.0 }]).collect();
        let mut c = Conv::new(&ir);
        let energy: f32 = ir.iter().map(|s| s[0] * s[0] + s[1] * s[1]).sum();
        let norm = (1.0 / energy.sqrt()).min(4.0);
        let x: Vec<f32> = (0..3000).map(|i| ((i * 7919) % 13) as f32 / 13.0 - 0.5).collect();
        let mut y = vec![[0.0f32; 2]; x.len()];
        for (i, v) in x.iter().enumerate() {
            y[i] = c.process([*v, *v]);
        }
        for n in [900usize, 1500, 2500] {
            let mut d = 0.0;
            for (k, h) in ir.iter().enumerate() {
                if n >= CONV_B + k {
                    d += x[n - CONV_B - k] * h[0] * norm;
                }
            }
            assert!((y[n][0] - d).abs() < 1e-3, "{n}: {} vs {d}", y[n][0]);
            assert!((y[n][1] - x[n - CONV_B - 3] * norm).abs() < 1e-3);
        }
    }
}

#[cfg(test)]
mod clear_tests {
    use super::*;
    #[test]
    fn clear_silences() {
        let mut f = Fdn::new(48000.0);
        for i in 0..48000 {
            f.process(if i == 100 { [1.0; 2] } else { [0.0; 2] }, 8, 10.0, 0.55, 1.0, 1.0, 0.55, false);
        }
        f.clear();
        let mut e = 0.0;
        for _ in 0..48000 {
            let y = f.process([0.0; 2], 8, 10.0, 0.55, 1.0, 1.0, 0.55, false);
            e += y[0] * y[0];
        }
        assert!(e < 1e-12, "{e}");
    }
}
