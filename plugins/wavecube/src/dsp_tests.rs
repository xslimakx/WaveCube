//! Tests of the audio engine: behaviour, click safety, stability and speed.

use super::*;
use crate::wave::{Preset, Pt, Wave};
use crate::{Slot, WaveCubeParams};
use nih_plug::prelude::Enum;

fn sine(n: usize, f: f32, sr: f32) -> Vec<f32> {
    (0..n).map(|i| (2.0 * PI * f * i as f32 / sr).sin() * 0.5).collect()
}

fn run(eng: &mut Engine, p: &Settings, input: &[f32], shared: &Shared) -> Vec<[f32; 2]> {
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(256) {
        eng.begin_block(p, &t, shared);
        for &x in chunk {
            out.push(eng.process([x, x], [0.0; 2], true, None, p, shared));
        }
        eng.end_block(shared);
    }
    out
}

fn settings() -> Settings {
    let mut s = Settings::from_params(&WaveCubeParams::default());
    s.safe_clip = false;
    s
}

/// every shaper in the chain
fn full() -> State {
    let mut st = State::default();
    st.present = vec![true; N_SHAPERS];
    st
}

fn only(p: &mut Settings, k: Kind) {
    for (i, s) in p.sh.iter_mut().enumerate() {
        s.on = i == k.index();
        s.smooth_ms = 0.0;
    }
}

fn rms(v: &[[f32; 2]]) -> f32 {
    (v.iter().map(|s| s[0] * s[0]).sum::<f32>() / v.len() as f32).sqrt()
}

/// largest jump between neighbouring samples
fn max_step(v: &[[f32; 2]]) -> f32 {
    v.windows(2).map(|w| (w[1][0] - w[0][0]).abs().max((w[1][1] - w[0][1]).abs())).fold(0.0, f32::max)
}

#[test]
fn everything_off_is_transparent() {
    let mut p = settings();
    for s in p.sh.iter_mut() {
        s.on = false;
    }
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&full());
    let x = sine(4800, 440.0, 48000.0);
    let y = run(&mut e, &p, &x, &shared);
    for (a, b) in x.iter().zip(y.iter()) {
        assert!((a - b[0]).abs() < 1e-6 && (a - b[1]).abs() < 1e-6);
    }
}

#[test]
fn removed_shapers_do_nothing() {
    let mut p = settings();
    for s in p.sh.iter_mut() {
        s.on = true;
    }
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    let mut st = full();
    st.present = vec![false; N_SHAPERS];
    e.bake(&st);
    let x = sine(4800, 440.0, 48000.0);
    let y = run(&mut e, &p, &x, &shared);
    assert!(x.iter().zip(y.iter()).all(|(a, b)| (a - b[0]).abs() < 1e-6));
}

#[test]
fn neutral_shapes_are_transparent() {
    let mut p = settings();
    let mut st = full();
    for (i, k) in Kind::ALL.iter().enumerate() {
        let y = match k {
            Kind::Noise | Kind::Crush | Kind::Drive => 0.0,
            Kind::Pan | Kind::Width => 0.5,
            _ => 1.0,
        };
        st.waves[i] = vec![Wave::flat(y); N_BANDS];
        p.sh[i].on = *k != Kind::Filter;
    }
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let x = sine(9600, 440.0, 48000.0);
    let y = run(&mut e, &p, &x, &shared);
    for (a, b) in x.iter().zip(y.iter()).skip(10) {
        assert!((a - b[0]).abs() < 1e-4, "{a} {}", b[0]);
    }
}

#[test]
fn volume_follows_the_shape() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    let mut st = full();
    st.waves[0][0] = Wave::preset(Preset::Square);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let x = vec![0.5f32; 24000];
    let y = run(&mut e, &p, &x, &shared);
    assert!((y[6000][0] - 0.5).abs() < 1e-3);
    assert!(y[18000][0].abs() < 1e-3, "{:?} {:?}", y[18000], y[17000]);
}

#[test]
fn switching_a_shaper_does_not_click() {
    // Volume at a flat 0 (silence): switching it on / off must glide, not jump
    let mut p = settings();
    only(&mut p, Kind::Volume);
    let mut st = full();
    st.waves[0][0] = Wave::flat(0.0);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    let mut out = Vec::new();
    for blk in 0..60 {
        p.sh[0].on = (blk / 20) % 2 == 0;
        e.begin_block(&p, &t, &shared);
        for _ in 0..256 {
            out.push(e.process([0.5, 0.5], [0.0; 2], true, None, &p, &shared));
        }
    }
    assert!(max_step(&out) < 0.01, "{}", max_step(&out));
}

#[test]
fn amount_mix_and_gain_changes_glide() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    let mut st = full();
    st.waves[0][0] = Wave::flat(0.0);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    let mut out = Vec::new();
    for blk in 0..40 {
        p.sh[0].amount = if blk < 20 { 0.0 } else { 1.0 };
        p.sh[0].mix = if blk < 30 { 1.0 } else { 0.0 };
        p.out_gain = if blk < 35 { 1.0 } else { 0.25 };
        e.begin_block(&p, &t, &shared);
        for _ in 0..256 {
            out.push(e.process([0.5, 0.5], [0.0; 2], true, None, &p, &shared));
        }
    }
    assert!(max_step(&out) < 0.01, "{}", max_step(&out));
}

#[test]
fn compressor_pulls_loud_parts_down_and_listens_to_the_sidechain() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    p.v_comp = true;
    p.v_thr = -20.0;
    p.v_ratio = 4.0;
    p.v_auto = false;
    let mut st = full();
    st.waves[0][0] = Wave::flat(1.0);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let x = sine(48000, 200.0, 48000.0);
    let y = run(&mut e, &p, &x, &shared);
    let pk = y[24000..].iter().map(|v| v[0].abs()).fold(0.0, f32::max);
    let db = 20.0 * (pk / 0.5).log10();
    assert!((-12.5..-8.5).contains(&db), "{db}");
    // sidechain: a quiet signal ducked by a loud sidechain
    p.v_sc = true;
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    let mut last = [0.0; 2];
    for _ in 0..(48000 / 256) {
        e.begin_block(&p, &t, &shared);
        for _ in 0..256 {
            last = e.process([0.05, 0.05], [0.9, 0.9], true, None, &p, &shared);
        }
    }
    assert!(last[0] < 0.02, "{}", last[0]);
}

#[test]
fn three_bands_sum_flat() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    p.sh[0].multiband = true;
    let mut st = full();
    st.waves[0] = vec![Wave::flat(1.0); N_BANDS];
    let shared = Shared::default();
    for f in [60.0, 200.0, 700.0, 2500.0, 9000.0] {
        let mut e = Engine::new(48000.0);
        e.bake(&st);
        let x = sine(48000, f, 48000.0);
        let y = run(&mut e, &p, &x, &shared);
        let pin = x[24000..].iter().map(|v| v * v).sum::<f32>();
        let pout = y[24000..].iter().map(|v| v[0] * v[0]).sum::<f32>();
        let db = 10.0 * (pout / pin).log10();
        assert!(db.abs() < 0.1, "{f} Hz: {db} dB");
    }
}

#[test]
fn solo_plays_one_band() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    p.sh[0].multiband = true;
    let mut st = full();
    st.waves[0] = vec![Wave::flat(1.0); N_BANDS];
    let shared = Shared::default();
    shared.solo[0].store(0b100, Ordering::Relaxed);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(24000, 80.0, 48000.0), &shared);
    assert!(rms(&y[12000..]) < 0.01);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(24000, 9000.0, 48000.0), &shared);
    assert!(rms(&y[12000..]) > 0.3);
}

#[test]
fn every_filter_type_closes_or_passes_sensibly() {
    let shared = Shared::default();
    for t in 0..FilterType::variants().len() {
        let ty = FilterType::from_index(t);
        let mut p = settings();
        only(&mut p, Kind::Filter);
        p.f_type = ty;
        p.f_lo = 200.0;
        p.f_res = 0.0;
        let mut st = full();
        st.waves[1][0] = Wave::flat(0.0);
        let mut e = Engine::new(48000.0);
        e.bake(&st);
        let hi = run(&mut e, &p, &sine(24000, 6000.0, 48000.0), &shared);
        let mut e = Engine::new(48000.0);
        e.bake(&st);
        let lo = run(&mut e, &p, &sine(24000, 30.0, 48000.0), &shared);
        let (h, l) = (rms(&hi[12000..]), rms(&lo[12000..]));
        assert!(h.is_finite() && l.is_finite());
        use FilterType::*;
        match ty {
            Lp6 => assert!(h < 0.05, "{ty:?} {h}"),
            Lp12 | Lp24 => assert!(h < 0.01, "{ty:?} {h}"),
            Hp12 | Hp24 => assert!(l < 0.03, "{ty:?} {l}"),
            Hp6 => assert!(l < 0.12, "{ty:?} {l}"),
            _ => {}
        }
    }
}

#[test]
fn filter_sweep_is_smooth() {
    // a fast sweep with high resonance must stay bounded and not crackle
    let shared = Shared::default();
    let mut p = settings();
    only(&mut p, Kind::Filter);
    p.sh[1].beats = 0.25;
    p.f_res = 0.9;
    p.f_type = FilterType::Lp24;
    let mut st = full();
    st.waves[1][0] = Wave::preset(Preset::Triangle);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(48000, 110.0, 48000.0), &shared);
    assert!(y.iter().all(|v| v[0].is_finite() && v[0].abs() < 4.0));
    assert!(max_step(&y[2000..]) < 0.6, "{}", max_step(&y[2000..]));
}

#[test]
fn envelope_opens_the_filter() {
    let shared = Shared::default();
    let mut p = settings();
    only(&mut p, Kind::Filter);
    p.f_lo = 200.0;
    p.sh[1].env_on = true;
    p.sh[1].env_amt = 1.0;
    let mut st = full();
    st.waves[1][0] = Wave::flat(0.0);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(24000, 5000.0, 48000.0), &shared);
    assert!(rms(&y[12000..]) > 0.15, "{}", rms(&y[12000..]));
}

#[test]
fn time_delays_by_the_shape() {
    let mut p = settings();
    only(&mut p, Kind::Time);
    let mut st = full();
    st.waves[4][0] = Wave::flat(0.5);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    // 1 bar at 120 = 2 s; half a cycle late = 1 s (the buffer must hold that much first)
    let mut x = vec![0.0f32; 120000];
    x[60000] = 1.0;
    let y = run(&mut e, &p, &x, &shared);
    let at = y.iter().position(|v| v[0] > 0.5).unwrap();
    assert!((at as i64 - 108000).abs() < 3, "{at}");
}

#[test]
fn time_with_tiny_delays_is_clean() {
    // 0 .. 3 samples of delay: no reads from the future (8 s old audio)
    let mut p = settings();
    only(&mut p, Kind::Time);
    p.t_range = TimeRange::Ms2;
    p.sh[4].beats = 0.25;
    let mut st = full();
    st.waves[4][0] = Wave::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 0.97, 0.0)]);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    // first fill the buffer with something loud and different, so a wrong read would show
    let mut x: Vec<f32> = (0..400_000).map(|i| if (i / 7) % 2 == 0 { 0.9 } else { -0.9 }).collect();
    x.extend(sine(48000, 200.0, 48000.0));
    let y = run(&mut e, &p, &x, &shared);
    let tail = &y[400_100..];
    assert!(max_step(tail) < 0.03, "{}", max_step(tail));
}

#[test]
fn time_does_not_replay_old_audio_after_a_restart() {
    let mut p = settings();
    only(&mut p, Kind::Time);
    let mut st = full();
    st.waves[4][0] = Wave::flat(0.0); // a whole cycle late
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    // loud audio while on, then off, then silence in and on again: nothing loud may come out
    let mut loud_after = 0.0f32;
    for blk in 0..800 {
        p.sh[4].on = !(300..400).contains(&blk);
        let inp = if blk < 300 { 0.8 } else { 0.0 };
        e.begin_block(&p, &t, &shared);
        for _ in 0..256 {
            let o = e.process([inp, inp], [0.0; 2], true, None, &p, &shared);
            if blk >= 400 {
                loud_after = loud_after.max(o[0].abs());
            }
        }
    }
    assert!(loud_after < 1e-3, "{loud_after}");
}

#[test]
fn stereo_phase_moves_the_channels_apart() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    p.sh[0].spread = 0.5;
    let mut st = full();
    st.waves[0][0] = Wave::preset(Preset::Square);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &vec![0.5; 24000], &shared);
    assert!(y[6000][0] > 0.45 && y[6000][1] < 0.05, "{:?}", y[6000]);
    assert!(y[18000][0] < 0.05 && y[18000][1] > 0.45, "{:?}", y[18000]);
}

#[test]
fn swing_delays_the_off_steps() {
    assert_eq!(swing_warp(0.3, 0.0, 16.0), 0.3);
    // grid 4: pairs of 0.5; with full swing the second step starts at 0.375 of the cycle
    let g = 4.0;
    assert!(swing_warp(0.36, 1.0, g) < 0.25);
    assert!(swing_warp(0.38, 1.0, g) >= 0.25);
    assert!((swing_warp(0.5, 1.0, g) - 0.5).abs() < 1e-6);
    let mut last = 0.0;
    for k in 0..1000 {
        let v = swing_warp(k as f32 / 1000.0, 0.6, 16.0);
        assert!(v >= last - 1e-6);
        last = v;
    }
}

#[test]
fn safety_clipper_never_passes_full_scale() {
    let mut p = settings();
    p.safe_clip = true;
    only(&mut p, Kind::Volume);
    p.v_trim = 8.0;
    let mut st = full();
    st.waves[0][0] = Wave::flat(1.0);
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(4800, 100.0, 48000.0), &shared);
    assert!(y.iter().all(|v| v[0].abs() <= 1.0));
    assert!((safe(0.5) - 0.5).abs() < 1e-9);
}

#[test]
fn noise_only_outputs_noise() {
    let mut p = settings();
    only(&mut p, Kind::Noise);
    p.n_only = true;
    let mut st = full();
    st.waves[5][0] = Wave::flat(1.0);
    let shared = Shared::default();
    for t in 0..NoiseType::variants().len() {
        p.n_type = NoiseType::from_index(t);
        let mut e = Engine::new(48000.0);
        e.bake(&st);
        let y = run(&mut e, &p, &vec![0.0; 24000], &shared);
        let r = rms(&y[4000..]);
        assert!(r > 0.003 && r < 0.6, "{:?} {r}", p.n_type);
    }
}

#[test]
fn crush_and_drive_change_the_sound_and_stay_bounded() {
    let shared = Shared::default();
    let x = sine(24000, 220.0, 48000.0);
    let mut p = settings();
    only(&mut p, Kind::Crush);
    p.c_bits = 3.0;
    let mut st = full();
    st.waves[6][0] = Wave::flat(1.0);
    st.waves[7][0] = Wave::flat(1.0);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &x, &shared);
    let diff = x.iter().zip(y.iter()).skip(2000).map(|(a, b)| (a - b[0]).abs()).fold(0.0, f32::max);
    assert!(diff > 0.05);
    for s in 0..DriveStyle::variants().len() {
        let mut p = settings();
        only(&mut p, Kind::Drive);
        p.d_style = DriveStyle::from_index(s);
        p.d_push = 0.7;
        let mut e = Engine::new(48000.0);
        e.bake(&st);
        let y = run(&mut e, &p, &x, &shared);
        let pk = y.iter().map(|v| v[0].abs()).fold(0.0, f32::max);
        assert!(pk.is_finite() && pk < 2.5 && pk > 0.01, "{:?} {pk}", p.d_style);
    }
}

#[test]
fn pan_moves_left_and_haas_delays() {
    let shared = Shared::default();
    let mut p = settings();
    only(&mut p, Kind::Pan);
    let mut st = full();
    st.waves[2][0] = Wave::flat(1.0);
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let y = run(&mut e, &p, &sine(4800, 440.0, 48000.0), &shared);
    assert!(y[2000][1].abs() < 1e-6);
    p.p_haas = 1.0;
    p.p_haas_ms = 10.0;
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let mut x = vec![0.0; 4800];
    x[1000] = 1.0;
    let y = run(&mut e, &p, &x, &shared);
    let l = y.iter().position(|v| v[0] > 0.5).unwrap();
    let r = y.iter().position(|v| v[1] > 0.5).unwrap();
    assert_eq!(l, 1000);
    assert!((r as i64 - 1480).abs() <= 1, "{r}");
}

#[test]
fn midi_restarts_and_one_shot_holds() {
    let mut p = settings();
    p.sh[0].trigger = Trigger::MidiOnce;
    p.sh[0].beats = 0.25;
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&full());
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    e.begin_block(&p, &t, &shared);
    e.process([0.1; 2], [0.0; 2], true, Some(60), &p, &shared);
    assert!(e.sh[0].phase < 0.001);
    for _ in 0..20000 {
        e.process([0.1; 2], [0.0; 2], true, None, &p, &shared);
    }
    assert!(e.sh[0].done);
    e.process([0.1; 2], [0.0; 2], true, Some(62), &p, &shared);
    assert!(!e.sh[0].done && e.sh[0].phase < 0.001);
}

#[test]
fn pitch_mode_runs_at_the_note() {
    let mut p = settings();
    p.sh[0].mode = Mode::Pitch;
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    e.begin_block(&p, &t, &shared);
    e.process([0.0; 2], [0.0; 2], true, Some(69), &p, &shared);
    assert!((e.sh[0].inc - 440.0 / 48000.0).abs() < 1e-9);
}

#[test]
fn midi_switch_loads_user_waves() {
    let mut p = settings();
    only(&mut p, Kind::Volume);
    p.midi_switch = true;
    let mut st = full();
    st.waves[0][0] = Wave::flat(1.0);
    st.slots[1] = Some(Slot { shaper: 0, band: 0, wave: Wave::flat(0.0) });
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    e.bake(&st);
    let t = Transport { tempo: 120.0, pos_beats: None, playing: false };
    e.begin_block(&p, &t, &shared);
    let mut a = [0.0; 2];
    for _ in 0..2000 {
        a = e.process([0.5; 2], [0.0; 2], true, None, &p, &shared);
    }
    assert!((a[0] - 0.5).abs() < 1e-3);
    e.process([0.5; 2], [0.0; 2], true, Some(62), &p, &shared);
    let b = e.process([0.5; 2], [0.0; 2], true, None, &p, &shared);
    assert!(b[0].abs() < 1e-3, "{}", b[0]);
    assert_eq!(shared.switched.load(Ordering::Relaxed) & 0xff, 1);
}

#[test]
fn sync_follows_the_song_position() {
    let p = settings();
    let shared = Shared::default();
    let mut e = Engine::new(48000.0);
    let t = Transport { tempo: 120.0, pos_beats: Some(10.25), playing: true };
    e.begin_block(&p, &t, &shared);
    assert!((e.sh[0].phase - 0.25).abs() < 1e-9);
}

#[test]
fn fuzz_never_explodes() {
    let shared = Shared::default();
    let mut rng = Rng(77);
    let mut st = full();
    for s in 0..N_SHAPERS {
        for b in 0..N_BANDS {
            st.waves[s][b] = Wave::steps(&(0..16).map(|_| rng.uni()).collect::<Vec<_>>());
        }
    }
    for round in 0..10 {
        let mut p = settings();
        for s in p.sh.iter_mut() {
            s.on = true;
            s.multiband = round % 2 == 1;
            s.smooth_ms = (round as f32) * 3.0;
            s.mode = [Mode::Beat, Mode::Hz, Mode::Pitch][round % 3];
            s.trigger = Trigger::from_index(round % 5);
            s.hz = [7.0, 4000.0][round % 2];
            s.env_on = round % 3 == 0;
            s.env_mult = round % 4 == 0;
            s.env_sc = round % 5 == 0;
            s.env_amt = if round % 2 == 0 { 2.0 } else { -2.0 };
            s.swing = 0.1 * round as f32;
            s.spread = 0.05 * round as f32;
        }
        p.v_comp = true;
        p.v_sc = round % 2 == 0;
        p.v_ratio = 100.0;
        p.f_res = 1.0;
        p.f_stereo = 60.0;
        p.f_type = FilterType::from_index((round * 2) % FilterType::variants().len());
        p.d_style = DriveStyle::from_index(round % 10);
        p.d_in = 36.0;
        p.d_push = 1.0;
        p.d_grip = 1.0;
        p.c_crunch = 27.0;
        p.c_bits = 1.0;
        p.c_jitter = 1.0;
        p.c_rate = 9.0;
        p.n_type = NoiseType::from_index(round % 8);
        p.p_haas = 1.0;
        p.p_haas_ms = 40.0;
        p.t_range = TimeRange::from_index(round % 8);
        let mut e = Engine::new(44100.0);
        e.bake(&st);
        let x: Vec<f32> = (0..30000).map(|_| rng.next()).collect();
        let t = Transport { tempo: 140.0, pos_beats: None, playing: false };
        let mut ok = true;
        for (i, chunk) in x.chunks(128).enumerate() {
            e.begin_block(&p, &t, &shared);
            for (j, &v) in chunk.iter().enumerate() {
                let note = if (i * 128 + j) % 3000 == 0 { Some(((i + j) % 128) as u8) } else { None };
                let o = e.process([v, -v], [v * 0.5, v], true, note, &p, &shared);
                ok &= o[0].is_finite() && o[1].is_finite() && o[0].abs() < 40.0 && o[1].abs() < 40.0;
            }
            e.end_block(&shared);
        }
        assert!(ok, "round {round}");
    }
}

#[test]
fn cheap_enough_with_everything_on() {
    let shared = Shared::default();
    let mut p = settings();
    for s in p.sh.iter_mut() {
        s.on = true;
        s.multiband = true;
        s.env_on = true;
        s.spread = 0.25;
        s.swing = 0.3;
    }
    p.v_comp = true;
    p.f_type = FilterType::PhN8;
    p.p_haas = 0.5;
    let mut e = Engine::new(48000.0);
    e.bake(&full());
    let x = sine(48000, 220.0, 48000.0);
    let t0 = std::time::Instant::now();
    let y = run(&mut e, &p, &x, &shared);
    let dt = t0.elapsed().as_secs_f64();
    eprintln!("1 s of audio, 8 shapers x 3 bands, everything on: {:.1} ms", dt * 1000.0);
    let mut p1 = settings();
    only(&mut p1, Kind::Volume);
    let mut e = Engine::new(48000.0);
    e.bake(&full());
    let t0 = std::time::Instant::now();
    run(&mut e, &p1, &x, &shared);
    eprintln!("1 s of audio, one Volume shaper: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
    assert!(y.len() == 48000);
    if !cfg!(debug_assertions) {
        assert!(dt < 0.35, "{dt}");
    }
}
