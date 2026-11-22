//! Noise sources (kinds::NOISES) with pitch: the generator runs at a variable rate and is read
//! with interpolation, so ±24 semitones change its spectrum like a resampled recording.

use super::blocks::{coef_hz, Rng, Svf, SvfCoef};
use std::f32::consts::PI;

pub const N_NOISES: usize = crate::kinds::NOISES.len();

/// fixed coefficients (at the engine rate)
#[derive(Clone, Copy)]
pub struct NoiseCoefs {
    sr: f32,
    bp3k: SvfCoef,
    bp5k: SvfCoef,
    bp1k: SvfCoef,
    bp4k: SvfCoef,
    hp2k: SvfCoef,
    hp7k: SvfCoef,
    lp6k: SvfCoef,
    a5k: f32,
    a9k: f32,
    a400: f32,
}

impl NoiseCoefs {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            bp3k: SvfCoef::new(3000.0, sr, 0.7),
            bp5k: SvfCoef::new(6500.0, sr, 0.8),
            bp1k: SvfCoef::new(1200.0, sr, 0.6),
            bp4k: SvfCoef::new(4500.0, sr, 1.0),
            hp2k: SvfCoef::new(2000.0, sr, 1.4),
            hp7k: SvfCoef::new(7000.0, sr, 1.4),
            lp6k: SvfCoef::new(6000.0, sr, 1.4),
            a5k: coef_hz(5000.0, sr),
            a9k: coef_hz(9000.0, sr),
            a400: coef_hz(400.0, sr),
        }
    }
}

/// One noise source (three of them make the stereo noise).
#[derive(Clone, Copy, Default)]
pub struct Gen {
    b: [f32; 7],
    brown: f32,
    prev: f32,
    prev2: f32,
    hold: f32,
    cnt: f32,
    rate: f32,
    crack: f32,
    lp: f32,
    lp2: f32,
    f1: Svf,
    f2: Svf,
    ph: [f32; 6],
    slow: f32,
    slow_t: f32,
    env: f32,
    // resampling
    acc: f32,
    s0: f32,
    s1: f32,
}

const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

impl Gen {
    #[inline]
    fn pink(&mut self, w: f32) -> f32 {
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }

    #[inline]
    fn metal(&mut self, sr: f32) -> f32 {
        let mut s = 0.0;
        for (p, f) in self.ph.iter_mut().zip(METAL.iter()) {
            *p = (*p + f * 2.0 / sr) % 1.0;
            s += if *p < 0.5 { 1.0 } else { -1.0 };
        }
        s / 6.0
    }

    /// slow random 0..1 (for wind, ocean, brush)
    #[inline]
    fn slow(&mut self, rng: &mut Rng, sr: f32, hz: f32) -> f32 {
        self.slow_t += hz / sr;
        if self.slow_t >= 1.0 {
            self.slow_t -= 1.0;
            self.hold = rng.uni();
        }
        self.slow += (self.hold - self.slow) * (hz * 4.0 / sr);
        self.slow
    }

    /// One sample of noise type `t` at the engine rate.
    #[inline]
    fn raw(&mut self, t: usize, rng: &mut Rng, c: &NoiseCoefs, hum: f32) -> f32 {
        let sr = c.sr;
        let w = rng.next();
        match t {
            0 => w * 0.5,
            1 => self.pink(w) * 1.2,
            2 => {
                self.brown = (self.brown + w * 0.02) / 1.02;
                self.brown * 3.0
            }
            3 => {
                let v = (w - self.prev) * 0.35;
                self.prev = w;
                v
            }
            4 => {
                let v = (w - 2.0 * self.prev + self.prev2) * 0.2;
                self.prev2 = self.prev;
                self.prev = w;
                v
            }
            5 => self.pink(w) * 0.8 + w * 0.15,
            6 => {
                // rain: drops + soft bed
                if rng.uni() < 400.0 / sr {
                    self.crack = rng.next();
                }
                self.crack *= 0.93;
                let drop = self.f1.run(&c.bp3k, self.crack).1;
                drop * 0.8 + self.pink(w) * 0.12
            }
            7 => {
                // wind: brown through a slowly wandering resonance
                let m = self.slow(rng, sr, 0.3);
                let f = 250.0 + 900.0 * m;
                let cf = SvfCoef::new(f, sr, 0.25);
                self.brown = (self.brown + w * 0.02) / 1.02;
                self.f1.run(&cf, self.brown * 3.0 + w * 0.05).1 * 0.6
            }
            8 => {
                // ocean: pink with slow swells
                let m = self.slow(rng, sr, 0.12);
                self.pink(w) * (0.15 + 1.1 * m * m)
            }
            9 => {
                // fire: pops and a low rumble
                if rng.uni() < 25.0 / sr {
                    self.crack = rng.next() * 1.2;
                }
                self.crack *= 0.6;
                self.brown = (self.brown + w * 0.02) / 1.02;
                self.crack + self.brown * 1.2
            }
            10 => {
                let p = self.pink(w);
                self.lp2 += (p - self.lp2) * c.a9k;
                self.lp += (self.lp2 - self.lp) * c.a400;
                (self.lp2 - self.lp) * 1.6
            }
            11 => {
                if rng.uni() < 30.0 / sr {
                    self.crack = rng.next() * 0.9;
                }
                self.crack *= 0.55;
                let hiss = self.pink(w) * 0.04;
                self.crack + hiss
            }
            12 | 13 => hum + w * 0.004,
            14 => {
                // radio static: bursts of noise and a drifting whistle
                let m = self.slow(rng, sr, 3.0);
                self.ph[0] = (self.ph[0] + (900.0 + 2500.0 * m) / sr) % 1.0;
                let whistle = (2.0 * PI * self.ph[0]).sin() * 0.05;
                w * (0.15 + 0.5 * m * m) + whistle
            }
            15 => {
                self.cnt += self.rate.max(200.0) / sr;
                if self.cnt >= 1.0 {
                    self.cnt -= 1.0;
                    self.hold = w * 0.5;
                    self.rate = 600.0 + rng.uni() * 7000.0;
                }
                self.hold
            }
            16 => {
                let m = self.metal(sr);
                self.f1.run(&c.hp7k, m).2 * 0.8
            }
            17 => {
                let m = self.metal(sr);
                let v = self.f1.run(&c.hp7k, m * 0.7 + w * 0.3).2;
                v * 0.8
            }
            18 => {
                let m = self.metal(sr);
                let v = self.f1.run(&c.hp2k, m + w * 0.1).2;
                self.f2.run(&c.lp6k, v).0 * 0.7
            }
            19 => {
                // shaker: band-passed noise with a fast random tremolo
                self.slow_t += 16.0 / sr;
                if self.slow_t >= 1.0 {
                    self.slow_t -= 1.0;
                    self.env = 0.5 + 0.5 * rng.uni();
                }
                self.lp *= 0.9995;
                let tr = self.env * (1.0 - self.slow_t).powi(3);
                self.f1.run(&c.bp5k, w).1 * tr * 1.6
            }
            20 => self.f1.run(&c.bp4k, w).1 * 0.9,
            21 => {
                // clap: bursts of band-passed noise
                if rng.uni() < 8.0 / sr {
                    self.env = 1.0;
                }
                self.env *= 0.9992;
                self.f1.run(&c.bp1k, w).1 * (0.1 + self.env) * 1.2
            }
            22 => {
                let m = self.slow(rng, sr, 1.5);
                let v = self.pink(w);
                self.f1.run(&c.hp2k, v).2 * (0.2 + m) * 1.5
            }
            _ => {
                self.lp += (w - self.lp) * c.a5k;
                (w - self.lp) * 0.7
            }
        }
    }

    /// One output sample at `rate` (1 = original pitch).
    #[inline]
    pub fn next(&mut self, t: usize, rng: &mut Rng, c: &NoiseCoefs, hum: f32, rate: f32) -> f32 {
        if (rate - 1.0).abs() < 1e-4 {
            return self.raw(t, rng, c, hum);
        }
        self.acc += rate;
        let mut guard = 0;
        while self.acc >= 1.0 && guard < 8 {
            self.acc -= 1.0;
            self.s0 = self.s1;
            self.s1 = self.raw(t, rng, c, hum);
            guard += 1;
        }
        self.acc = self.acc.min(1.0);
        self.s0 + (self.s1 - self.s0) * self.acc
    }
}

/// Mains hum at `hz` with harmonics.
#[inline]
pub fn hum(ph: f32) -> f32 {
    let h = 2.0 * PI * ph;
    (h.sin() + 0.5 * (2.0 * h).sin() + 0.3 * (3.0 * h).sin() + 0.15 * (5.0 * h).sin()) * 0.3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_noise_makes_sound_at_every_pitch() {
        let c = NoiseCoefs::new(48000.0);
        for t in 0..N_NOISES {
            for rate in [0.25f32, 1.0, 4.0] {
                let mut g = Gen::default();
                let mut rng = Rng(99 + t as u32);
                let mut ph = 0.0f32;
                let mut e = 0.0;
                let mut mx = 0.0f32;
                for _ in 0..48000 {
                    ph = (ph + 50.0 / 48000.0) % 1.0;
                    let v = g.next(t, &mut rng, &c, hum(ph), rate);
                    assert!(v.is_finite());
                    e += v * v;
                    mx = mx.max(v.abs());
                }
                let r = (e / 48000.0).sqrt();
                assert!(r > 0.003 && mx < 4.0, "{} rate {rate}: rms {r} max {mx}", crate::kinds::NOISES[t].1);
            }
        }
    }
}
