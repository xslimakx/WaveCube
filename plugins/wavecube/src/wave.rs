//! The drawn LFO shapes: a list of points (x and y in 0..1) joined by curved segments.
//!
//! The first point always sits at x = 0 and the last at x = 1. Two points may share an x: the
//! shape then jumps (steps, gates). Every point carries the bend of the segment that starts at it.

use serde::{Deserialize, Serialize};

pub const MAX_POINTS: usize = 192;
/// Resolution of the table the audio thread reads.
pub const TABLE: usize = 2048;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
    /// bend of the segment to the next point, -1 .. 1 (0 = straight)
    pub c: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32, c: f32) -> Self {
        Self { x, y, c }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Wave {
    pub pts: Vec<Pt>,
}

/// Bent interpolation 0..1 -> 0..1. c > 0 starts slow, c < 0 starts fast.
#[inline]
pub fn bend(t: f32, c: f32) -> f32 {
    if c.abs() < 1e-4 {
        t
    } else if c > 0.0 {
        t.powf(2f32.powf(c * 3.0))
    } else {
        1.0 - (1.0 - t).powf(2f32.powf(-c * 3.0))
    }
}

/// Pen shapes painted into one grid cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pen {
    Step,
    RampUp,
    RampDown,
    Arc,
    SCurve,
    Triangle,
}

impl Pen {
    pub const ALL: [Pen; 6] = [Pen::Step, Pen::RampUp, Pen::RampDown, Pen::Arc, Pen::SCurve, Pen::Triangle];
    pub fn name(self) -> &'static str {
        match self {
            Pen::Step => "Step",
            Pen::RampUp => "Ramp up",
            Pen::RampDown => "Ramp down",
            Pen::Arc => "Arc",
            Pen::SCurve => "S-curve",
            Pen::Triangle => "Triangle",
        }
    }
}

/// The built-in shapes of the editor's SHAPE menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Square,
    Pump,
    PumpHard,
    Gate8,
    Gate16,
    TapeStop,
    Stutter,
    Bounce,
    Flat,
    Zero,
    HalfTime,
    Scratch,
    GateTriplet,
    Ramps16,
    Swell,
    Duck8,
    TrimIn,
    TrimOut,
    TapeStart,
}

impl Preset {
    pub const ALL: [Preset; 23] = [
        Preset::Sine,
        Preset::Triangle,
        Preset::SawUp,
        Preset::SawDown,
        Preset::Square,
        Preset::Pump,
        Preset::PumpHard,
        Preset::Gate8,
        Preset::Gate16,
        Preset::TapeStop,
        Preset::Stutter,
        Preset::Bounce,
        Preset::Flat,
        Preset::Zero,
        Preset::HalfTime,
        Preset::Scratch,
        Preset::GateTriplet,
        Preset::Ramps16,
        Preset::Swell,
        Preset::Duck8,
        Preset::TrimIn,
        Preset::TrimOut,
        Preset::TapeStart,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Preset::Sine => "Sine",
            Preset::Triangle => "Triangle",
            Preset::SawUp => "Saw up",
            Preset::SawDown => "Saw down",
            Preset::Square => "Square",
            Preset::Pump => "Pump",
            Preset::PumpHard => "Pump hard",
            Preset::Gate8 => "Gate 1/8",
            Preset::Gate16 => "Gate 1/16",
            Preset::TapeStop => "Tape stop",
            Preset::Stutter => "Stutter",
            Preset::Bounce => "Bounce",
            Preset::Flat => "Flat (top)",
            Preset::Zero => "Flat (bottom)",
            Preset::HalfTime => "Half-time",
            Preset::Scratch => "Scratch",
            Preset::GateTriplet => "Gate triplets",
            Preset::Ramps16 => "Ramps 1/16",
            Preset::Swell => "Swell",
            Preset::Duck8 => "Duck 1/8",
            Preset::TrimIn => "Fade in",
            Preset::TrimOut => "Fade out",
            Preset::TapeStart => "Tape start",
        }
    }
}

impl Wave {
    pub fn from(pts: &[Pt]) -> Self {
        let mut w = Self { pts: pts.to_vec() };
        w.sanitize();
        w
    }

    pub fn flat(y: f32) -> Self {
        Self::from(&[Pt::new(0.0, y, 0.0), Pt::new(1.0, y, 0.0)])
    }

    pub fn preset(p: Preset) -> Self {
        let s = 0.42; // bend that makes four segments look like a sine
        match p {
            Preset::Sine => Self::from(&[
                Pt::new(0.0, 0.5, -s),
                Pt::new(0.25, 1.0, s),
                Pt::new(0.5, 0.5, -s),
                Pt::new(0.75, 0.0, s),
                Pt::new(1.0, 0.5, 0.0),
            ]),
            Preset::Triangle => Self::from(&[Pt::new(0.0, 0.0, 0.0), Pt::new(0.5, 1.0, 0.0), Pt::new(1.0, 0.0, 0.0)]),
            Preset::SawUp => Self::from(&[Pt::new(0.0, 0.0, 0.0), Pt::new(1.0, 1.0, 0.0)]),
            Preset::SawDown => Self::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 0.0, 0.0)]),
            Preset::Square => Self::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(0.5, 1.0, 0.0), Pt::new(0.5, 0.0, 0.0), Pt::new(1.0, 0.0, 0.0)]),
            Preset::Pump => Self::from(&[Pt::new(0.0, 0.0, -0.55), Pt::new(0.42, 1.0, 0.0), Pt::new(1.0, 1.0, 0.0)]),
            Preset::PumpHard => Self::from(&[Pt::new(0.0, 0.0, 0.0), Pt::new(0.08, 0.0, -0.7), Pt::new(0.55, 1.0, 0.0), Pt::new(1.0, 1.0, 0.0)]),
            Preset::Gate8 => Self::steps(&[1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0]),
            Preset::Gate16 => Self::steps(&[1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            Preset::TapeStop => Self::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(0.5, 1.0, -0.35), Pt::new(1.0, 0.0, 0.0)]),
            Preset::Stutter => Self::from(&[
                Pt::new(0.0, 1.0, 0.0),
                Pt::new(0.5, 0.75, 0.0),
                Pt::new(0.5, 1.0, 0.0),
                Pt::new(0.75, 0.875, 0.0),
                Pt::new(0.75, 1.0, 0.0),
                Pt::new(0.875, 0.9375, 0.0),
                Pt::new(0.875, 1.0, 0.0),
                Pt::new(1.0, 0.9375, 0.0),
            ]),
            Preset::Bounce => Self::from(&[
                Pt::new(0.0, 1.0, 0.6),
                Pt::new(0.4, 0.0, -0.6),
                Pt::new(0.6, 0.55, 0.6),
                Pt::new(0.8, 0.0, -0.6),
                Pt::new(0.9, 0.25, 0.6),
                Pt::new(1.0, 0.0, 0.0),
            ]),
            Preset::Flat => Self::flat(1.0),
            Preset::Zero => Self::flat(0.0),
            Preset::HalfTime => Self::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 0.5, 0.0)]),
            Preset::Scratch => Self::from(&[
                Pt::new(0.0, 1.0, 0.0),
                Pt::new(0.5, 1.0, 0.4),
                Pt::new(0.625, 0.6, -0.4),
                Pt::new(0.75, 0.9, 0.4),
                Pt::new(0.875, 0.5, -0.4),
                Pt::new(1.0, 1.0, 0.0),
            ]),
            Preset::GateTriplet => Self::steps(&[1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0]),
            Preset::Ramps16 => {
                let mut w = Self::flat(0.0);
                for k in 0..16 {
                    w.paint(k as f32 / 16.0, (k + 1) as f32 / 16.0, 1.0, Pen::RampDown);
                }
                w
            }
            Preset::Swell => Self::from(&[Pt::new(0.0, 0.0, 0.6), Pt::new(0.9, 1.0, 0.0), Pt::new(1.0, 0.0, 0.0)]),
            Preset::TrimIn => Self::from(&[Pt::new(0.0, 0.0, -0.3), Pt::new(0.12, 1.0, 0.0), Pt::new(1.0, 1.0, 0.0)]),
            Preset::TrimOut => Self::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(0.88, 1.0, 0.3), Pt::new(1.0, 0.0, 0.0)]),
            Preset::TapeStart => Self::from(&[Pt::new(0.0, 0.0, 0.35), Pt::new(0.5, 1.0, 0.0), Pt::new(1.0, 1.0, 0.0)]),
            Preset::Duck8 => {
                let mut w = Self::flat(1.0);
                for k in 0..8 {
                    w.paint(k as f32 / 8.0, (k + 1) as f32 / 8.0, 1.0, Pen::Arc);
                }
                w
            }
        }
    }

    /// Equal steps (gate patterns).
    pub fn steps(levels: &[f32]) -> Self {
        let n = levels.len().max(1);
        let mut pts = Vec::with_capacity(2 * n);
        for (i, &l) in levels.iter().enumerate() {
            pts.push(Pt::new(i as f32 / n as f32, l, 0.0));
            pts.push(Pt::new((i + 1) as f32 / n as f32, l, 0.0));
        }
        let mut w = Self { pts };
        w.sanitize();
        w
    }

    /// Keeps the shape valid whatever happened to it (old states, bad edits): sorted, in range,
    /// ends at 0 and 1, not too many points, no exact duplicates.
    pub fn sanitize(&mut self) {
        for p in &mut self.pts {
            p.x = if p.x.is_finite() { p.x.clamp(0.0, 1.0) } else { 0.0 };
            p.y = if p.y.is_finite() { p.y.clamp(0.0, 1.0) } else { 1.0 };
            p.c = if p.c.is_finite() { p.c.clamp(-1.0, 1.0) } else { 0.0 };
        }
        self.pts.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        self.pts.dedup_by(|b, a| (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6);
        // never more than two points at the same x
        let mut i = 2;
        while i < self.pts.len() {
            if (self.pts[i].x - self.pts[i - 2].x).abs() < 1e-6 {
                self.pts.remove(i - 1);
            } else {
                i += 1;
            }
        }
        if self.pts.is_empty() {
            self.pts = vec![Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 1.0, 0.0)];
        }
        if self.pts[0].x > 0.0 {
            let y = self.pts[0].y;
            self.pts.insert(0, Pt::new(0.0, y, 0.0));
        }
        if self.pts.last().unwrap().x < 1.0 {
            let y = self.pts.last().unwrap().y;
            self.pts.push(Pt::new(1.0, y, 0.0));
        }
        if self.pts.len() == 1 {
            let y = self.pts[0].y;
            self.pts.push(Pt::new(1.0, y, 0.0));
        }
        if self.pts.len() > MAX_POINTS {
            let last = *self.pts.last().unwrap();
            self.pts.truncate(MAX_POINTS - 1);
            self.pts.push(last);
        }
    }

    /// Value at x (0..1). At a jump the value right of it is returned.
    pub fn eval(&self, x: f32) -> f32 {
        self.eval_side(x, false)
    }

    /// Value at x; `left` picks the value just left of a jump.
    pub fn eval_side(&self, x: f32, left: bool) -> f32 {
        let p = &self.pts;
        let x = x.clamp(0.0, 1.0);
        // segment i .. i+1 with p[i].x <= x <= p[i+1].x
        let n = p.len();
        let mut i = 0;
        if left {
            while i + 2 < n && p[i + 1].x < x {
                i += 1;
            }
        } else {
            while i + 2 < n && p[i + 1].x <= x {
                i += 1;
            }
        }
        let (a, b) = (p[i], p[i + 1]);
        let w = b.x - a.x;
        if w <= 1e-7 {
            return if left { a.y } else { b.y };
        }
        let t = ((x - a.x) / w).clamp(0.0, 1.0);
        a.y + (b.y - a.y) * bend(t, a.c)
    }

    /// Fills the lookup table the audio thread reads.
    pub fn bake(&self, out: &mut [f32; TABLE]) {
        let p = &self.pts;
        let mut seg = 0;
        for (k, o) in out.iter_mut().enumerate() {
            let x = k as f32 / TABLE as f32;
            while seg + 2 < p.len() && p[seg + 1].x <= x {
                seg += 1;
            }
            let (a, b) = (p[seg], p[seg + 1]);
            let w = b.x - a.x;
            *o = if w <= 1e-7 { b.y } else { a.y + (b.y - a.y) * bend(((x - a.x) / w).clamp(0.0, 1.0), a.c) };
        }
    }

    /// Paints a flat step at level y from x0 to x1 (the pencil tool).
    pub fn paint_step(&mut self, x0: f32, x1: f32, y: f32) {
        self.paint(x0, x1, y, Pen::Step);
    }

    /// Paints a pen shape into x0 .. x1 with height y.
    pub fn paint(&mut self, x0: f32, x1: f32, y: f32, pen: Pen) {
        let (x0, x1) = (x0.clamp(0.0, 1.0), x1.clamp(0.0, 1.0));
        if x1 <= x0 {
            return;
        }
        let y = y.clamp(0.0, 1.0);
        let yl = self.eval_side(x0, true);
        let yr = self.eval_side(x1, false);
        let eps = 1e-6;
        let mut out: Vec<Pt> = self.pts.iter().copied().filter(|p| p.x < x0 - eps).collect();
        if x0 > eps {
            out.push(Pt::new(x0, yl, 0.0));
        }
        let xm = 0.5 * (x0 + x1);
        match pen {
            Pen::Step => {
                out.push(Pt::new(x0, y, 0.0));
                out.push(Pt::new(x1, y, 0.0));
            }
            Pen::RampUp => {
                out.push(Pt::new(x0, 0.0, 0.0));
                out.push(Pt::new(x1, y, 0.0));
            }
            Pen::RampDown => {
                out.push(Pt::new(x0, y, 0.0));
                out.push(Pt::new(x1, 0.0, 0.0));
            }
            Pen::Arc => {
                out.push(Pt::new(x0, 0.0, -0.55));
                out.push(Pt::new(x1, y, 0.0));
            }
            Pen::SCurve => {
                out.push(Pt::new(x0, 0.0, 0.5));
                out.push(Pt::new(xm, 0.5 * y, -0.5));
                out.push(Pt::new(x1, y, 0.0));
            }
            Pen::Triangle => {
                out.push(Pt::new(x0, 0.0, 0.0));
                out.push(Pt::new(xm, y, 0.0));
                out.push(Pt::new(x1, 0.0, 0.0));
            }
        }
        if x1 < 1.0 - eps {
            out.push(Pt::new(x1, yr, self.seg_bend_at(x1)));
        }
        out.extend(self.pts.iter().copied().filter(|p| p.x > x1 + eps));
        // a step that continues the one next to it: drop the shared corner points
        self.pts = out;
        self.merge_flat_runs();
        self.sanitize();
    }

    fn seg_bend_at(&self, x: f32) -> f32 {
        let p = &self.pts;
        for i in 0..p.len() - 1 {
            if p[i].x <= x && x < p[i + 1].x {
                return p[i].c;
            }
        }
        0.0
    }

    /// Removes middle points of flat runs (three points in a row at the same level).
    fn merge_flat_runs(&mut self) {
        let mut i = 1;
        while i + 1 < self.pts.len() {
            let (a, b, c) = (self.pts[i - 1], self.pts[i], self.pts[i + 1]);
            let flat = (a.y - b.y).abs() < 1e-6 && (b.y - c.y).abs() < 1e-6;
            if flat {
                self.pts.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Adds a point; returns its index.
    pub fn insert(&mut self, x: f32, y: f32) -> Option<usize> {
        if self.pts.len() >= MAX_POINTS {
            return None;
        }
        let x = x.clamp(0.0, 1.0);
        let i = self.pts.iter().position(|p| p.x > x).unwrap_or(self.pts.len() - 1).max(1);
        let c = self.pts[i - 1].c;
        self.pts.insert(i, Pt::new(x, y.clamp(0.0, 1.0), c));
        Some(i)
    }

    /// Removes a point (not the end points).
    pub fn remove(&mut self, i: usize) {
        if i > 0 && i + 1 < self.pts.len() {
            self.pts.remove(i);
        }
    }

    /// Moves a point, keeping the order; the end points only move up and down.
    pub fn move_pt(&mut self, i: usize, x: f32, y: f32) {
        let n = self.pts.len();
        if i >= n {
            return;
        }
        let x = if i == 0 {
            0.0
        } else if i == n - 1 {
            1.0
        } else {
            x.clamp(self.pts[i - 1].x, self.pts[i + 1].x)
        };
        self.pts[i].x = x;
        self.pts[i].y = y.clamp(0.0, 1.0);
    }

    pub fn invert(&mut self) {
        for p in &mut self.pts {
            p.y = 1.0 - p.y;
        }
    }

    /// Mirror in time.
    pub fn reverse(&mut self) {
        let n = self.pts.len();
        let old = self.pts.clone();
        self.pts = (0..n)
            .map(|k| {
                let src = old[n - 1 - k];
                // the segment that now starts at this point was old segment n-2-k, played backwards
                let c = if k + 1 < n { -old[n - 2 - k].c } else { 0.0 };
                Pt::new(1.0 - src.x, src.y, c)
            })
            .collect();
        self.sanitize();
    }

    /// Doubles the speed: the shape twice in one cycle.
    pub fn double(&mut self) {
        self.repeat(2);
    }

    /// The shape n times in one cycle (2x, 3x).
    pub fn repeat(&mut self, n: usize) {
        if self.pts.len() * n > MAX_POINTS || n < 2 {
            return;
        }
        let k = 1.0 / n as f32;
        let mut out = Vec::with_capacity(self.pts.len() * n);
        for r in 0..n {
            out.extend(self.pts.iter().map(|p| Pt::new(p.x * k + r as f32 * k, p.y, p.c)));
        }
        self.pts = out;
        self.sanitize();
    }

    /// A random shape on the grid: random levels, slopes and bends. `keep_x` keeps the points
    /// where they are and only moves them up and down.
    pub fn randomize(&mut self, grid: usize, seed: u32, keep_x: bool) {
        let mut r = seed.max(1);
        let mut rnd = move || {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            r as f32 / u32::MAX as f32
        };
        if keep_x {
            for p in self.pts.iter_mut() {
                p.y = rnd();
                p.c = (rnd() - 0.5) * 1.2;
            }
            return;
        }
        let n = grid.clamp(2, 64);
        let mut pts = Vec::new();
        for k in 0..n {
            let x0 = k as f32 / n as f32;
            match (rnd() * 4.0) as u32 {
                0 => {
                    let y = rnd();
                    pts.push(Pt::new(x0, y, 0.0));
                    pts.push(Pt::new(x0 + 0.999 / n as f32, y, 0.0));
                }
                1 => pts.push(Pt::new(x0, rnd(), (rnd() - 0.5) * 1.4)),
                2 => {
                    pts.push(Pt::new(x0, 0.0, -0.5));
                }
                _ => pts.push(Pt::new(x0, rnd(), 0.0)),
            }
        }
        let y0 = pts[0].y;
        pts.push(Pt::new(1.0, y0, 0.0));
        self.pts = pts;
        self.sanitize();
    }

    /// Moves the shape in time by dx (wraps around).
    pub fn shift(&mut self, dx: f32) {
        // rebuild from the points: rotate them and re-add the wrap point
        let mut pts: Vec<Pt> = self.pts[..self.pts.len() - 1]
            .iter()
            .map(|p| Pt::new((p.x + dx).rem_euclid(1.0), p.y, p.c))
            .collect();
        pts.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let y0 = self.eval((-dx).rem_euclid(1.0));
        let y1 = self.eval_side((-dx).rem_euclid(1.0), true);
        let mut out = vec![Pt::new(0.0, y0, 0.0)];
        out.extend(pts.into_iter().filter(|p| p.x > 1e-6));
        out.push(Pt::new(1.0, y1, 0.0));
        self.pts = out;
        self.sanitize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_follows_points() {
        let w = Wave::preset(Preset::Triangle);
        assert!((w.eval(0.0) - 0.0).abs() < 1e-6);
        assert!((w.eval(0.25) - 0.5).abs() < 1e-6);
        assert!((w.eval(0.5) - 1.0).abs() < 1e-6);
        assert!((w.eval(1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn jumps_left_and_right() {
        let w = Wave::preset(Preset::Square);
        assert_eq!(w.eval_side(0.5, true), 1.0);
        assert_eq!(w.eval_side(0.5, false), 0.0);
    }

    #[test]
    fn bend_is_monotonic_and_ends_fixed() {
        for c in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            assert!((bend(0.0, c)).abs() < 1e-6 && (bend(1.0, c) - 1.0).abs() < 1e-6);
            let mut last = 0.0;
            for k in 0..=100 {
                let v = bend(k as f32 / 100.0, c);
                assert!(v >= last - 1e-6);
                last = v;
            }
        }
    }

    #[test]
    fn paint_steps_make_a_gate() {
        let mut w = Wave::flat(1.0);
        w.paint_step(0.25, 0.5, 0.0);
        assert_eq!(w.eval(0.1), 1.0);
        assert_eq!(w.eval(0.3), 0.0);
        assert_eq!(w.eval(0.49), 0.0);
        assert_eq!(w.eval(0.6), 1.0);
        // painting the next cell continues the step without a ramp
        w.paint_step(0.5, 0.75, 0.0);
        assert_eq!(w.eval(0.6), 0.0);
        assert_eq!(w.eval(0.8), 1.0);
        // painting over again keeps it compact
        for _ in 0..20 {
            w.paint_step(0.25, 0.5, 0.0);
        }
        assert!(w.pts.len() < 10, "{:?}", w.pts);
    }

    #[test]
    fn pens_paint_into_one_cell() {
        for pen in Pen::ALL {
            let mut w = Wave::flat(1.0);
            w.paint(0.25, 0.5, 0.8, pen);
            assert_eq!(w.eval(0.1), 1.0, "{pen:?}");
            assert_eq!(w.eval(0.6), 1.0, "{pen:?}");
            let peak = (1..100).map(|k| w.eval(0.25 + 0.25 * k as f32 / 100.0)).fold(0.0f32, f32::max);
            assert!((peak - 0.8).abs() < 0.02, "{pen:?} {peak}");
        }
    }

    #[test]
    fn repeat_and_random_stay_valid() {
        let mut w = Wave::preset(Preset::Pump);
        w.repeat(3);
        assert!((w.eval(1.0 / 6.0) - Wave::preset(Preset::Pump).eval(0.5)).abs() < 1e-3);
        for seed in 1..50 {
            let mut r = Wave::flat(0.5);
            r.randomize(16, seed, false);
            assert_eq!(r.pts.first().unwrap().x, 0.0);
            assert_eq!(r.pts.last().unwrap().x, 1.0);
            r.randomize(16, seed, true);
            assert!(r.pts.iter().all(|p| (0.0..=1.0).contains(&p.y)));
        }
    }

    #[test]
    fn reverse_twice_is_identity() {
        let w = Wave::preset(Preset::Pump);
        let mut r = w.clone();
        r.reverse();
        assert!((r.eval(0.8) - w.eval(0.2)).abs() < 1e-4);
        r.reverse();
        for k in 0..50 {
            let x = k as f32 / 50.0;
            assert!((r.eval(x) - w.eval(x)).abs() < 1e-4);
        }
    }

    #[test]
    fn bake_matches_eval() {
        let w = Wave::preset(Preset::Sine);
        let mut t = Box::new([0.0; TABLE]);
        w.bake(&mut t);
        for k in (0..TABLE).step_by(37) {
            assert!((t[k] - w.eval(k as f32 / TABLE as f32)).abs() < 1e-4);
        }
    }

    #[test]
    fn sanitize_fixes_garbage() {
        let mut w = Wave { pts: vec![Pt::new(0.7, f32::NAN, 3.0), Pt::new(0.2, 2.0, 0.0)] };
        w.sanitize();
        assert_eq!(w.pts.first().unwrap().x, 0.0);
        assert_eq!(w.pts.last().unwrap().x, 1.0);
        assert!(w.pts.iter().all(|p| (0.0..=1.0).contains(&p.y) && (-1.0..=1.0).contains(&p.c)));
    }
}
