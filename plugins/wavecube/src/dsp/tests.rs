//! Engine tests: every item, the chain, bands, LFOs, envelopes, click safety, stability, speed.

use super::*;
use crate::wave::{Preset, Pt, Wave};
use crate::{Custom, State};

struct Rig {
    sr: f32,
    st: State,
    ctl: Controls,
    e: Engine,
    shared: Shared,
    t: Transport,
}

impl Rig {
    fn new() -> Self {
        Self::at(48000.0)
    }
    fn at(sr: f32) -> Self {
        let shared = Shared::default();
        shared.sr.store(sr, Ordering::Relaxed);
        Self { sr, st: State::empty(), ctl: Controls::default(), e: Engine::new(sr), shared, t: Transport { tempo: 120.0, pos_beats: None, playing: false } }
    }
    /// adds an item at the end of the chain with its default knobs; returns its slot
    fn add(&mut self, kind: Kind) -> usize {
        let s = self.st.free_slot().unwrap();
        self.st.put(s, kind);
        self.st.chain.push(s as u8);
        self.ctl.defaults_for(s, kind);
        s
    }
    fn knob(&mut self, s: usize, i: usize, plain: f32) {
        let d = self.st.slots[s].kind.knobs()[i];
        self.ctl.knob[s][i] = d.norm(plain);
    }
    fn wave(&mut self, s: usize, lane: usize, w: Wave) {
        self.st.waves[s][lane] = vec![w; N_BANDS];
    }
    fn ready(&mut self) {
        self.st.sanitize();
        self.e = Engine::new(self.sr);
        self.e.prepare(&self.st, &self.shared);
    }
    fn run(&mut self, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(256) {
            self.e.begin_block(&self.ctl, &self.t, &self.shared);
            for x in chunk {
                out.push(self.e.process(*x, [0.0; 2], true, None, &self.shared));
            }
            self.e.end_block(&self.shared);
            self.advance(chunk.len());
        }
        out
    }
    /// a playing host moves on
    fn advance(&mut self, n: usize) {
        if let Some(p) = self.t.pos_beats.as_mut() {
            *p += n as f64 * self.t.tempo / 60.0 / self.sr as f64;
        }
    }
    fn run_sc(&mut self, input: &[[f32; 2]], sc: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut out = Vec::with_capacity(input.len());
        for (ci, chunk) in input.chunks(256).enumerate() {
            self.e.begin_block(&self.ctl, &self.t, &self.shared);
            for (j, x) in chunk.iter().enumerate() {
                out.push(self.e.process(*x, sc[ci * 256 + j], true, None, &self.shared));
            }
            self.e.end_block(&self.shared);
            self.advance(chunk.len());
        }
        out
    }
}

fn sine(n: usize, f: f32) -> Vec<[f32; 2]> {
    (0..n).map(|i| (2.0 * PI * f * i as f32 / 48000.0).sin() * 0.5).map(|v| [v, v]).collect()
}

fn dc(n: usize, v: f32) -> Vec<[f32; 2]> {
    vec![[v, v]; n]
}

fn rms(v: &[[f32; 2]]) -> f32 {
    (v.iter().map(|s| s[0] * s[0]).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

fn max_step(v: &[[f32; 2]]) -> f32 {
    v.windows(2).map(|w| (w[1][0] - w[0][0]).abs().max((w[1][1] - w[0][1]).abs())).fold(0.0, f32::max)
}

fn peak(v: &[[f32; 2]]) -> f32 {
    v.iter().map(|s| s[0].abs().max(s[1].abs())).fold(0.0, f32::max)
}

#[test]
fn empty_chain_is_bit_exact() {
    let mut r = Rig::new();
    r.ready();
    let x = sine(4800, 440.0);
    let y = r.run(&x);
    assert!(x.iter().zip(y.iter()).all(|(a, b)| a == b));
}

#[test]
fn neutral_items_are_transparent() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.wave(v, 0, Wave::flat(1.0));
    let p = r.add(Kind::Pan);
    r.wave(p, 0, Wave::flat(0.5));
    let w = r.add(Kind::Width);
    r.wave(w, 0, Wave::flat(0.5));
    let t = r.add(Kind::Time);
    r.wave(t, 0, Wave::flat(1.0));
    let d = r.add(Kind::Drive);
    r.wave(d, 0, Wave::flat(0.0));
    let rv = r.add(Kind::Reverb);
    r.wave(rv, 0, Wave::flat(0.0));
    let n = r.add(Kind::Noise);
    r.wave(n, 0, Wave::flat(0.0));
    let s = r.add(Kind::Scope);
    let _ = s;
    let l = r.add(Kind::Liquid);
    r.st.slots[l].l_type = 0;
    r.ready();
    let x = sine(9600, 440.0);
    let y = r.run(&x);
    for (a, b) in x.iter().zip(y.iter()).skip(10) {
        assert!((a[0] - b[0]).abs() < 1e-4, "{} {}", a[0], b[0]);
    }
}

#[test]
fn the_same_shaper_twice_works() {
    let mut one = Rig::new();
    let a = one.add(Kind::Drive);
    one.wave(a, 0, Wave::flat(0.6));
    one.ready();
    let x = sine(9600, 220.0);
    let y1 = one.run(&x);
    let mut two = Rig::new();
    for _ in 0..2 {
        let s = two.add(Kind::Drive);
        two.wave(s, 0, Wave::flat(0.6));
    }
    two.ready();
    let y2 = two.run(&x);
    let diff = y1.iter().zip(y2.iter()).map(|(a, b)| (a[0] - b[0]).abs()).fold(0.0, f32::max);
    assert!(diff > 0.01, "the second drive does nothing");
    assert!(y2.iter().all(|v| v[0].is_finite()));
}

#[test]
fn volume_follows_the_shape_and_the_offset() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].lfo[0][0].smooth = 0.0;
    r.wave(v, 0, Wave::preset(Preset::Square));
    r.ready();
    let y = r.run(&dc(24000, 0.5));
    // 1/4 at 120 = 24000 samples: first half open, second closed
    assert!((y[6000][0] - 0.5).abs() < 1e-3);
    assert!(y[18000][0].abs() < 1e-3);
    // the offset lifts the whole shape
    r.ctl.tile[v][0] = 0.5;
    r.ready();
    let y = r.run(&dc(24000, 0.5));
    assert!((y[18000][0] - 0.25).abs() < 1e-3, "{}", y[18000][0]);
}

#[test]
fn switching_items_on_and_off_does_not_click() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.wave(v, 0, Wave::flat(0.0));
    r.ready();
    let mut out = Vec::new();
    for blk in 0..60 {
        r.ctl.on[v] = (blk / 20) % 2 == 0;
        out.extend(r.run(&dc(256, 0.5)));
    }
    assert!(max_step(&out) < 0.01, "{}", max_step(&out));
}

#[test]
fn knob_and_mix_changes_glide() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.wave(v, 0, Wave::flat(1.0));
    r.ready();
    let mut out = Vec::new();
    for blk in 0..40 {
        r.knob(v, 0, if blk < 20 { 0.0 } else { -24.0 });
        r.ctl.mix[v] = if blk < 30 { 1.0 } else { 0.0 };
        r.ctl.out_db = if blk < 35 { 0.0 } else { -12.0 };
        out.extend(r.run(&dc(256, 0.5)));
    }
    assert!(max_step(&out) < 0.01, "{}", max_step(&out));
}

#[test]
fn bands_have_their_own_lfo_unless_linked() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].multiband = true;
    r.st.slots[v].link[0] = false;
    for b in 0..3 {
        r.st.slots[v].lfo[0][b].length = [7, 8, 9][b];
    }
    r.ready();
    r.run(&dc(1000, 0.1));
    let incs: Vec<f64> = (0..3).map(|b| r.e.sl[v].lfo[0][b].inc).collect();
    assert!((incs[1] / incs[0] - 2.0).abs() < 1e-9 && (incs[2] / incs[1] - 2.0).abs() < 1e-9, "{incs:?}");
    // linked: every band runs like the mid band
    r.st.slots[v].link[0] = true;
    r.ready();
    r.run(&dc(1000, 0.1));
    let incs: Vec<f64> = (0..3).map(|b| r.e.sl[v].lfo[0][b].inc).collect();
    assert!(incs.iter().all(|i| (i - incs[1]).abs() < 1e-12));
}

#[test]
fn bands_sum_flat_at_every_slope_and_solo_works() {
    for slope in 0..3u8 {
        let mut r = Rig::new();
        let v = r.add(Kind::Volume);
        r.st.slots[v].multiband = true;
        r.st.slots[v].slope = slope;
        r.wave(v, 0, Wave::flat(1.0));
        r.ready();
        for f in [80.0, 700.0, 6000.0] {
            r.ready();
            let x = sine(48000, f);
            let y = r.run(&x);
            let db = 20.0 * (rms(&y[24000..]) / rms(&x[24000..])).log10();
            assert!(db.abs() < 0.2, "slope {slope} {f} Hz: {db}");
        }
    }
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].multiband = true;
    r.wave(v, 0, Wave::flat(1.0));
    r.ready();
    r.shared.solo[v].store(0b100, Ordering::Relaxed);
    let y = r.run(&sine(24000, 80.0));
    assert!(rms(&y[12000..]) < 0.01);
}

#[test]
fn one_shot_stops_at_the_end_marker() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    let lfo = &mut r.st.slots[v].lfo[0][0];
    lfo.looping = false;
    lfo.end = 0.5;
    lfo.smooth = 0.0;
    r.wave(v, 0, Wave::preset(Preset::SawUp));
    r.ready();
    let y = r.run(&dc(48000, 1.0));
    // the saw rises to 0.5 and stays there
    assert!((y[40000][0] - 0.5).abs() < 0.01, "{}", y[40000][0]);
    assert!(r.e.sl[v].lfo[0][0].done);
}

#[test]
fn pitch_mode_runs_at_the_note_plus_transpose() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].lfo[0][0].mode = 2;
    r.st.slots[v].lfo[0][0].transpose = 12.0;
    r.ready();
    r.e.begin_block(&r.ctl, &r.t, &r.shared);
    r.e.process([0.0; 2], [0.0; 2], true, Some(69), &r.shared);
    assert!((r.e.sl[v].lfo[0][0].inc - 880.0 / 48000.0).abs() < 1e-9);
}

#[test]
fn midi_and_audio_triggers_restart() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].lfo[0][0].trigger = 1;
    let w = r.add(Kind::Width);
    r.st.slots[w].lfo[0][0].trigger = 2;
    r.ready();
    r.e.begin_block(&r.ctl, &r.t, &r.shared);
    for _ in 0..5000 {
        r.e.process([0.0; 2], [0.0; 2], true, None, &r.shared);
    }
    r.e.process([0.0; 2], [0.0; 2], true, Some(60), &r.shared);
    assert!(r.e.sl[v].lfo[0][0].phase < 1e-3);
    assert!(r.e.sl[w].lfo[0][0].phase > 0.01);
    // a hit on the input
    for i in 0..400 {
        r.e.process(if i > 100 { [0.8; 2] } else { [0.0; 2] }, [0.0; 2], true, None, &r.shared);
    }
    assert!(r.e.sl[w].lfo[0][0].phase < 0.02, "{}", r.e.sl[w].lfo[0][0].phase);
}

#[test]
fn audio_trigger_shift_and_sidechain() {
    let mut r = Rig::new();
    let w = r.add(Kind::Width);
    r.st.slots[w].lfo[0][0].trigger = 2;
    r.st.trig.sidechain = true;
    r.st.trig.shift = 2.0;
    r.ready();
    r.e.begin_block(&r.ctl, &r.t, &r.shared);
    for _ in 0..3000 {
        r.e.process([0.9; 2], [0.0; 2], true, None, &r.shared);
    }
    // the main input is loud but the trigger listens to the sidechain only
    assert!(r.e.sl[w].lfo[0][0].phase > 0.05);
    let mut at = None;
    for i in 0..300 {
        r.e.process([0.0; 2], [0.9; 2], true, None, &r.shared);
        if r.e.sl[w].lfo[0][0].phase < 1e-3 && at.is_none() {
            at = Some(i);
        }
    }
    // 2 ms later than the hit
    let at = at.expect("no trigger");
    assert!((90..=100).contains(&at), "{at}");
}

#[test]
fn envelope_depth_shift_free_and_sidechain() {
    let mut r = Rig::new();
    let f = r.add(Kind::Filter);
    r.wave(f, 0, Wave::flat(0.0));
    r.wave(f, 1, Wave::flat(0.0));
    let e = &mut r.st.slots[f].env[0][0];
    e.on = true;
    e.amount = 0.5;
    e.depth = 1; // ×2: full range
    r.ready();
    let y = r.run(&sine(24000, 6000.0));
    assert!(rms(&y[12000..]) > 0.2, "{}", rms(&y[12000..]));
    // shift alone opens it without any input
    let e = &mut r.st.slots[f].env[0][0];
    e.amount = 0.0;
    e.shift = 1.0;
    r.ready();
    r.run(&dc(2000, 0.0));
    assert!(r.e.sl[f].val[0][0][0] > 0.99);
    // sidechain: the filter opens on the sidechain, not on the signal
    let e = &mut r.st.slots[f].env[0][0];
    e.shift = 0.0;
    e.amount = 1.0;
    e.input = 2;
    r.ready();
    let x = sine(24000, 6000.0);
    let quiet = r.run_sc(&x, &dc(24000, 0.0));
    assert!(rms(&quiet[12000..]) < 0.02);
    r.ready();
    let loud = r.run_sc(&x, &sine(24000, 100.0).iter().map(|v| [v[0] * 1.8, v[1] * 1.8]).collect::<Vec<_>>());
    assert!(rms(&loud[12000..]) > 0.2);
}

#[test]
fn every_filter_type_slope_and_flavour_is_sane() {
    for warm in [false, true] {
        for slope in 0..3u8 {
            for ty in 0..5u8 {
                let mut r = Rig::new();
                let f = r.add(Kind::Filter);
                r.st.slots[f].f_type = ty;
                r.st.slots[f].f_slope = slope;
                r.st.slots[f].f_warm = warm;
                r.knob(f, 1, 0.6);
                r.wave(f, 0, Wave::flat(0.45)); // ~1 kHz
                r.wave(f, 1, Wave::flat(0.0));
                r.ready();
                let hi = r.run(&sine(24000, 12000.0));
                r.ready();
                let lo = r.run(&sine(24000, 60.0));
                let (h, l) = (rms(&hi[12000..]), rms(&lo[12000..]));
                assert!(h.is_finite() && l.is_finite() && peak(&hi) < 3.0);
                match ty {
                    0 => assert!(h < 0.1, "LP {slope} {warm}: {h}"),
                    1 => assert!(l < 0.12, "HP {slope} {warm}: {l}"),
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn liquid_flanger_and_phaser() {
    for t in [1u8, 2] {
        for neg in [false, true] {
            let mut r = Rig::new();
            let l = r.add(Kind::Liquid);
            r.st.slots[l].l_type = t;
            r.st.slots[l].l_neg = neg;
            r.st.slots[l].l_stages = 3;
            r.wave(l, 1, Wave::flat(1.0)); // full feedback
            r.ready();
            let x = sine(48000, 330.0);
            let y = r.run(&x);
            assert!(peak(&y) < 3.0 && y.iter().all(|v| v[0].is_finite()));
            let diff = x.iter().zip(y.iter()).map(|(a, b)| (a[0] - b[0]).abs()).fold(0.0, f32::max);
            assert!(diff > 0.02, "liquid {t} {neg} does nothing");
        }
    }
}

#[test]
fn reverb_tails_clear_and_gate() {
    let mut r = Rig::new();
    let rv = r.add(Kind::Reverb);
    r.wave(rv, 0, Wave::flat(1.0));
    r.wave(rv, 1, Wave::flat(1.0));
    r.knob(rv, 5, 0.0); // dry off
    r.knob(rv, 6, 1.0);
    r.ready();
    let mut x = dc(48000, 0.0);
    x[100] = [1.0, 1.0];
    let y = r.run(&x);
    assert!(rms(&y[2000..20000]) > 1e-4, "no tail");
    // clear tails: the volume shape drops to 0 half way -> nothing left after that
    let mut r2 = Rig::new();
    let rv = r2.add(Kind::Reverb);
    r2.st.slots[rv].r_clear = true;
    r2.st.slots[rv].lfo[1][0].smooth = 0.0;
    r2.st.slots[rv].r_type = 8; // endless
    r2.wave(rv, 0, Wave::flat(1.0));
    r2.wave(rv, 1, Wave::preset(Preset::Square));
    r2.knob(rv, 5, 0.0);
    r2.knob(rv, 6, 1.0);
    r2.st.slots[rv].lfo[1][0].length = 5; // 1 bar = 2 s
    r2.ready();
    let mut x = dc(150000, 0.0);
    x[100] = [1.0, 1.0];
    let y = r2.run(&x);
    // second cycle, open again: the tail was cleared at the hard drop
    assert!(rms(&y[97000..140000]) < 1e-5, "{}", rms(&y[97000..140000]));
}

#[test]
fn user_impulse_response_convolves() {
    let mut r = Rig::new();
    let rv = r.add(Kind::Reverb);
    r.st.slots[rv].r_type = 1000;
    r.wave(rv, 0, Wave::flat(1.0));
    r.wave(rv, 1, Wave::flat(1.0));
    r.knob(rv, 5, 0.0);
    r.knob(rv, 6, 1.0);
    r.knob(rv, 3, 1.0);
    let ir: Vec<[f32; 2]> = (0..2000).map(|i| if i == 500 { [1.0, 1.0] } else { [0.0, 0.0] }).collect();
    r.shared.irs.lock().push(Arc::new(media::Sample { data: ir, sr: 48000.0 }));
    r.ready();
    let mut x = dc(10000, 0.0);
    x[3000] = [1.0, 1.0];
    let y = r.run(&x);
    let at = y.iter().position(|v| v[0].abs() > 0.3).unwrap();
    assert_eq!(at, 3000 + 500 + reverb::CONV_B);
}

#[test]
fn noise_volume_pitch_and_user_files() {
    let mut r = Rig::new();
    let n = r.add(Kind::Noise);
    r.st.slots[n].n_only = true;
    r.wave(n, 0, Wave::flat(1.0));
    r.ready();
    let a = r.run(&dc(24000, 0.0));
    assert!(rms(&a[4000..]) > 0.01);
    // pitch down: darker (fewer sign changes)
    let zc = |v: &[[f32; 2]]| v.windows(2).filter(|w| (w[0][0] > 0.0) != (w[1][0] > 0.0)).count();
    r.wave(n, 1, Wave::flat(0.0));
    r.ready();
    let low = r.run(&dc(24000, 0.0));
    assert!(zc(&low[4000..]) * 3 < zc(&a[4000..]), "{} {}", zc(&low[4000..]), zc(&a[4000..]));
    // a user noise file plays
    r.wave(n, 1, Wave::flat(0.5));
    r.st.slots[n].n_type = 1000;
    let data: Vec<[f32; 2]> = (0..4800).map(|i| [((i as f32) * 0.05).sin() * 0.5; 2]).collect();
    r.shared.noises.lock().push(Arc::new(media::Sample { data, sr: 48000.0 }));
    r.shared.media_version.fetch_add(1, Ordering::AcqRel);
    r.ready();
    let u = r.run(&dc(24000, 0.0));
    assert!(rms(&u[4000..]) > 0.05);
}

#[test]
fn crush_bits_and_resample_lanes() {
    let mut r = Rig::new();
    let c = r.add(Kind::Crush);
    r.wave(c, 0, Wave::flat(1.0));
    r.wave(c, 1, Wave::flat(1.0));
    r.ready();
    let x = sine(24000, 220.0);
    let clean = r.run(&x);
    let d = x.iter().zip(clean.iter()).skip(100).map(|(a, b)| (a[0] - b[0]).abs()).fold(0.0, f32::max);
    assert!(d < 1e-3, "clean crush changes the sound: {d}");
    r.wave(c, 0, Wave::flat(0.15));
    r.ready();
    let bits = r.run(&x);
    let levels: std::collections::BTreeSet<i32> = bits[4000..].iter().map(|v| (v[0] * 40.0).round() as i32).collect();
    assert!(levels.len() < 12, "{}", levels.len());
    r.wave(c, 0, Wave::flat(1.0));
    r.wave(c, 1, Wave::flat(0.4));
    r.ready();
    let rs = r.run(&x);
    let holds = rs.windows(2).filter(|w| (w[0][0] - w[1][0]).abs() < 1e-3).count();
    assert!(holds > 10000);
}

#[test]
fn compressor_tool_and_sidechain() {
    let mut r = Rig::new();
    let c = r.add(Kind::Compressor);
    r.knob(c, 0, -20.0);
    r.knob(c, 1, 4.0);
    r.st.slots[c].comp_auto = false;
    r.ready();
    let x = sine(48000, 200.0);
    let y = r.run(&x);
    let db = 20.0 * (peak(&y[24000..]) / 0.5).log10();
    assert!((-12.5..-8.5).contains(&db), "{db}");
    r.st.slots[c].comp_input = 2;
    r.ready();
    let sc: Vec<[f32; 2]> = sine(24000, 100.0).iter().map(|v| [v[0] * 1.8, v[1] * 1.8]).collect();
    let y = r.run_sc(&dc(24000, 0.05), &sc);
    assert!(y[23000][0] < 0.02, "{}", y[23000][0]);
}

#[test]
fn time_is_clean_and_forgets() {
    let mut r = Rig::new();
    let t = r.add(Kind::Time);
    r.st.slots[t].t_range = 5; // 2.5 ms
    r.st.slots[t].lfo[0][0].length = 7;
    r.wave(t, 0, Wave::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 0.97, 0.0)]));
    r.ready();
    let mut x: Vec<[f32; 2]> = (0..400_000).map(|i| if (i / 7) % 2 == 0 { [0.9; 2] } else { [-0.9; 2] }).collect();
    x.extend(sine(48000, 200.0));
    let y = r.run(&x);
    assert!(max_step(&y[400_100..]) < 0.03, "{}", max_step(&y[400_100..]));
    // restart: no old audio
    let mut r = Rig::new();
    let t = r.add(Kind::Time);
    r.wave(t, 0, Wave::flat(0.0));
    r.ready();
    let mut loud_after = 0.0f32;
    for blk in 0..800 {
        r.ctl.on[t] = !(300..400).contains(&blk);
        let inp = if blk < 300 { 0.8 } else { 0.0 };
        let y = r.run(&dc(256, inp));
        if blk >= 400 {
            loud_after = loud_after.max(peak(&y));
        }
    }
    assert!(loud_after < 1e-3, "{loud_after}");
}

#[test]
fn stereo_offset_and_swing() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].lfo[0][0].spread = 180.0;
    r.st.slots[v].lfo[0][0].smooth = 0.0;
    r.wave(v, 0, Wave::preset(Preset::Square));
    r.ready();
    let y = r.run(&dc(24000, 0.5));
    assert!(y[6000][0] > 0.45 && y[6000][1] < 0.05);
    assert!(y[18000][0] < 0.05 && y[18000][1] > 0.45);
    assert!(swing_warp(0.36, 1.0, 4.0) < 0.25 && swing_warp(0.38, 1.0, 4.0) >= 0.25);
}

#[test]
fn midi_switch_loads_custom_waves() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.st.slots[v].lfo[0][0].smooth = 0.0;
    r.wave(v, 0, Wave::flat(1.0));
    r.st.custom[1] = Some(Custom { slot: v as u8, lane: 0, band: 0, kind: Kind::Volume, wave: Wave::flat(0.0) });
    r.ctl.midi_switch = true;
    r.ready();
    r.e.begin_block(&r.ctl, &r.t, &r.shared);
    let mut a = [0.0; 2];
    for _ in 0..2000 {
        a = r.e.process([0.5; 2], [0.0; 2], true, None, &r.shared);
    }
    assert!((a[0] - 0.5).abs() < 1e-3);
    r.e.process([0.5; 2], [0.0; 2], true, Some(62), &r.shared);
    let b = r.e.process([0.5; 2], [0.0; 2], true, None, &r.shared);
    assert!(b[0].abs() < 1e-3, "{}", b[0]);
    assert_eq!(r.shared.switched.load(Ordering::Relaxed) & 0xff, 1);
}

#[test]
fn scope_tool_fills_the_ring() {
    let mut r = Rig::new();
    let s = r.add(Kind::Scope);
    r.shared.view.store(s as u32, Ordering::Relaxed);
    r.ready();
    r.run(&sine(4096, 100.0));
    let ring = r.shared.ring.lock();
    assert!(ring.0.iter().any(|v| v[0].abs() > 0.4));
}

#[test]
fn safety_clipper_holds_full_scale() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    r.wave(v, 0, Wave::flat(1.0));
    r.knob(v, 0, 18.0);
    r.ctl.safe_clip = true;
    r.ready();
    let y = r.run(&sine(4800, 100.0));
    assert!(peak(&y) <= 1.0);
}

#[test]
fn heavy_buffers_arrive_from_the_gui_side() {
    // the audio thread passes dry until the big buffers are handed over, then uses them
    let mut r = Rig::new();
    let t = r.add(Kind::Time);
    r.wave(t, 0, Wave::flat(0.5));
    r.st.sanitize();
    let p = crate::WaveCubeParams::default();
    *p.state.write() = r.st.clone();
    r.e = Engine::new(48000.0);
    r.shared.bump_settings();
    r.shared.bump_waves();
    r.e.sync(&p, &r.shared);
    assert!(r.e.sl[t].heavy.is_none());
    let mut provided = [0u64; N_SLOTS];
    provide_heavy(&r.st, &r.shared, &mut provided);
    r.e.sync(&p, &r.shared);
    assert!(r.e.sl[t].heavy.is_some());
    assert!(r.shared.heavy_in.lock().is_empty());
}

#[test]
fn fuzz_every_item_everywhere() {
    let mut rng = Rng(77);
    for round in 0..6 {
        let mut r = Rig::new();
        for i in 0..crate::N_SLOTS {
            let kind = Kind::ALL[(i + round) % Kind::ALL.len()];
            let s = r.add(kind);
            let set = &mut r.st.slots[s];
            set.multiband = (i + round) % 2 == 0;
            set.slope = (round % 3) as u8;
            set.f_type = (i % 5) as u8;
            set.f_slope = (round % 3) as u8;
            set.f_warm = round % 2 == 0;
            set.d_style = (i % 12) as u8;
            set.n_type = ((i * 5 + round) % noise::N_NOISES) as u16;
            set.r_type = ((i + round) % crate::kinds::REVERBS.len()) as u16;
            set.l_type = (i % 3) as u8;
            set.comp_input = (i % 3) as u8;
            for l in 0..2 {
                set.link[l] = round % 2 == 1;
                for b in 0..3 {
                    let lf = &mut set.lfo[l][b];
                    lf.mode = ((i + b) % 3) as u8;
                    lf.hz = [3.0, 2000.0][round % 2];
                    lf.trigger = ((i + l) % 3) as u8;
                    lf.looping = (i + b) % 4 != 0;
                    lf.swing = 0.3;
                    lf.spread = 90.0;
                    let e = &mut set.env[l][b];
                    e.on = (i + l) % 2 == 0;
                    e.amount = -2.0 + (round as f32) * 0.8;
                    e.depth = (round % 4) as u8;
                    e.input = (i % 3) as u8;
                    e.adaptive = true;
                }
            }
            for l in 0..2 {
                r.st.waves[s][l] = (0..3).map(|_| Wave::steps(&(0..8).map(|_| rng.uni()).collect::<Vec<_>>())).collect();
            }
            for k in 0..N_KNOBS {
                r.ctl.knob[s][k] = rng.uni();
            }
        }
        r.ready();
        let mut ok = true;
        for blk in 0..120 {
            r.e.begin_block(&r.ctl, &r.t, &r.shared);
            for j in 0..128 {
                let v = rng.next();
                let note = if (blk * 128 + j) % 3000 == 0 { Some(((blk + j) % 128) as u8) } else { None };
                let o = r.e.process([v, -v], [v * 0.5, v], true, note, &r.shared);
                ok &= o[0].is_finite() && o[1].is_finite() && o[0].abs() < 1000.0;
            }
            r.e.end_block(&r.shared);
        }
        assert!(ok, "round {round}");
    }
}

#[test]
fn cpu_budget() {
    // a busy chain: 12 items, all multiband
    let mut r = Rig::new();
    for kind in [Kind::Volume, Kind::Filter, Kind::Pan, Kind::Width, Kind::Time, Kind::Noise, Kind::Crush, Kind::Drive, Kind::Reverb, Kind::Liquid, Kind::Compressor, Kind::Filter] {
        let s = r.add(kind);
        r.st.slots[s].multiband = true;
        for l in 0..2 {
            for b in 0..3 {
                r.st.slots[s].env[l][b].on = true;
                r.st.slots[s].lfo[l][b].spread = 45.0;
            }
        }
    }
    r.ready();
    let x = sine(48000, 220.0);
    let t0 = std::time::Instant::now();
    let y = r.run(&x);
    let dt = t0.elapsed().as_secs_f64();
    eprintln!("1 s of audio, 12 items x 3 bands with envelopes: {:.1} ms", dt * 1000.0);
    let mut one = Rig::new();
    one.add(Kind::Volume);
    one.ready();
    let t0 = std::time::Instant::now();
    one.run(&x);
    eprintln!("1 s of audio, one Volume: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(y.len(), 48000);
    if !cfg!(debug_assertions) {
        assert!(dt < 0.6, "{dt}");
    }
}


/// Controls from a preset's normalized parameter map.
fn controls_of(d: &crate::presets::PresetData) -> Controls {
    let mut c = Controls::default();
    for s in 0..N_SLOTS {
        let g = |f: &str, def: f32| d.params.get(&crate::presets::slot_id(f, s)).copied().unwrap_or(def);
        c.on[s] = g("on", 1.0) > 0.5;
        c.tile[s] = [g("ta", 0.5) * 2.0 - 1.0, g("tb", 0.5) * 2.0 - 1.0];
        c.mix[s] = g("mix", 1.0);
        c.band_mix[s] = [g("mxl", 1.0), g("mxm", 1.0), g("mxh", 1.0)];
        for k in 0..N_KNOBS {
            c.knob[s][k] = g(&format!("k{k}"), 0.5);
        }
    }
    c
}

#[test]
fn every_factory_preset_plays_clean() {
    let mut x = sine(48000 * 2, 110.0);
    for (i, s) in x.iter_mut().enumerate() {
        // a kick every beat on top of the sine
        let t = (i % 24000) as f32 / 48000.0;
        let k = (2.0 * PI * 60.0 * t).sin() * (-t * 20.0).exp() * 0.6;
        s[0] += k;
        s[1] += k;
    }
    for d in crate::presets::factory() {
        let mut r = Rig::new();
        r.st = d.state.clone();
        r.ctl = controls_of(&d);
        r.t = Transport { tempo: 120.0, pos_beats: Some(0.0), playing: true };
        r.ready();
        let y = r.run(&x);
        assert!(y.iter().all(|s| s[0].is_finite() && s[1].is_finite()), "{}: not finite", d.name);
        let pk = peak(&y);
        assert!(pk < 4.0, "{}: peak {pk}", d.name);
        assert!(rms(&y[48000..]) > 1e-4, "{}: silent", d.name);
    }
}


mod audit;
