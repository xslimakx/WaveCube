//! Audit: clicks on every kind of change, every sample rate, block sizes, silence, extremes.

use super::*;
use crate::presets::{factory, PresetData};
use crate::WaveCubeParams;
use std::time::Instant;

/// Flush-to-zero, as the plugin wrapper sets it around every process call.
fn ftz() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut m: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut m);
        std::arch::asm!("ldmxcsr [{}]", in(reg) &(m | 0x8040));
    }
}

fn sine_at(n: usize, f: f32, sr: f32, a: f32) -> Vec<[f32; 2]> {
    (0..n).map(|i| (2.0 * PI * f * i as f32 / sr).sin() * a).map(|v| [v, v * 0.9]).collect()
}

/// A drum-ish loop: kick every beat, hats, a chord (the demo input).
fn music(n: usize, sr: f32) -> Vec<[f32; 2]> {
    let beat = (sr * 0.5) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let tb = (i % beat) as f32 / sr;
            let kf = 50.0 + 120.0 * (-tb * 30.0).exp();
            let kick = (2.0 * PI * kf * tb).sin() * (-tb * 7.0).exp() * 0.7;
            let mut pad = 0.0;
            for f in [220.0, 261.63, 329.63, 392.0] {
                pad += (2.0 * PI * f * t).sin() * 0.06;
            }
            // a "hat": a fast decaying 7.3 kHz tone (the same at every sample rate, unlike noise)
            let h = (2.0 * PI * 7300.0 * t).sin();
            let off = (i % beat) > beat / 2;
            let hat = if off { h * (-((i % (beat / 2)) as f32 / sr) * 60.0).exp() * 0.15 } else { 0.0 };
            [kick + pad + hat, kick + pad * 0.95 + hat * 1.1]
        })
        .collect()
}

fn rig_for(d: &PresetData, sr: f32) -> Rig {
    let mut r = Rig::at(sr);
    r.st = d.state.clone();
    r.ctl = super::controls_of(d);
    r.t = Transport { tempo: 120.0, pos_beats: Some(0.0), playing: true };
    r.ready();
    r
}

fn rms_db(v: &[[f32; 2]]) -> f32 {
    20.0 * rms(v).max(1e-9).log10()
}

/// Every preset sounds the same (level) at every sample rate: catches anything tied to 48 kHz.
#[test]
fn every_preset_at_every_sample_rate() {
    let mut bad = Vec::new();
    for d in factory() {
        let x48 = music(48000 * 2, 48000.0);
        let mut r = rig_for(&d, 48000.0);
        let y48 = r.run(&x48);
        let ref_db = rms_db(&y48[48000..]);
        for sr in [22050.0f32, 44100.0, 96000.0, 192000.0] {
            let n = (sr * 2.0) as usize;
            let x = music(n, sr);
            let mut r = rig_for(&d, sr);
            let y = r.run(&x);
            let fin = y.iter().all(|s| s[0].is_finite() && s[1].is_finite());
            let db = rms_db(&y[n / 2..]);
            let pk = peak(&y);
            let tol = if sr < 30000.0 { 4.5 } else { 3.0 };
            if !fin || pk > 4.0 || (db - ref_db).abs() > tol {
                bad.push(format!("{} @ {sr}: {db:.1} dB vs {ref_db:.1} dB, peak {pk:.2}, finite {fin}", d.name));
            }
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// The host's block size does not change the sound.
#[test]
fn block_size_does_not_matter() {
    let mut bad = Vec::new();
    let x = music(48000, 48000.0);
    for d in factory().iter().step_by(3) {
        let mut outs = Vec::new();
        for bs in [1usize, 37, 256, 2048] {
            let mut r = rig_for(d, 48000.0);
            let mut y = Vec::with_capacity(x.len());
            for chunk in x.chunks(bs) {
                r.e.begin_block(&r.ctl, &r.t, &r.shared);
                for s in chunk {
                    y.push(r.e.process(*s, [0.0; 2], true, None, &r.shared));
                }
                r.e.end_block(&r.shared);
                r.advance(chunk.len());
            }
            outs.push(rms_db(&y[24000..]));
        }
        let (lo, hi) = outs.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        if hi - lo > 1.0 {
            bad.push(format!("{}: {outs:?}", d.name));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// Second difference: big at clicks, small for smooth audio.
fn d2(v: &[[f32; 2]]) -> f32 {
    v.windows(3).map(|w| (w[2][0] - 2.0 * w[1][0] + w[0][0]).abs().max((w[2][1] - 2.0 * w[1][1] + w[0][1]).abs())).fold(0.0, f32::max)
}

/// Changes of every kind while audio plays: switching on / off, mix, every knob, the lane offsets,
/// a new shape, structural settings (type, slope, multiband, chain order), removing an item.
#[test]
fn no_clicks_on_any_change() {
    let mut bad = Vec::new();
    for &kind in Kind::ALL.iter() {
        if matches!(kind, Kind::Noise | Kind::Crush) {
            continue; // noisy by nature: a click hides in them
        }
        let mut r = Rig::new();
        let s = r.add(kind);
        let v2 = r.add(Kind::Volume);
        r.wave(v2, 0, Wave::flat(1.0));
        for l in 0..kind.n_lanes() {
            r.wave(s, l, Wave::flat(0.5));
        }
        r.st.slots[s].f_slope = 1;
        r.ready();
        let params = WaveCubeParams::default();
        *params.state.write() = r.st.clone();
        let mut prov = [0u64; crate::N_SLOTS];
        let x = sine_at(48000 * 4, 110.0, 48000.0, 0.4);
        let mut y = Vec::new();
        let mut events = Vec::new();
        let mut pos = 0;
        let blocks = x.len() / 256;
        for blk in 0..blocks {
            let t = blk * 256;
            let ev = blk % 24 == 12 && blk > 24;
            if ev {
                events.push(t);
                let k = blk / 24;
                match k % 8 {
                    0 => r.ctl.on[s] = !r.ctl.on[s],
                    1 => r.ctl.mix[s] = if r.ctl.mix[s] > 0.5 { 0.2 } else { 1.0 },
                    2 => {
                        for i in 0..N_KNOBS {
                            r.ctl.knob[s][i] = 1.0 - r.ctl.knob[s][i];
                        }
                    }
                    3 => r.ctl.tile[s][0] = if r.ctl.tile[s][0] > 0.0 { -0.3 } else { 0.4 },
                    _ => {
                        let mut st = r.st.clone();
                        match k % 8 {
                            4 => {
                                for l in 0..kind.n_lanes() {
                                    st.waves[s][l] = vec![Wave::flat(if k % 16 < 8 { 0.9 } else { 0.2 }); 3];
                                }
                            }
                            5 => {
                                let set = &mut st.slots[s];
                                set.f_type = (set.f_type + 1) % 5;
                                set.d_style = (set.d_style + 5) % 12;
                                set.l_type = (set.l_type + 1) % 3;
                                set.r_type = (set.r_type + 3) % 14;
                                set.p_law = (set.p_law + 1) % 5;
                            }
                            6 => st.slots[s].multiband = !st.slots[s].multiband,
                            _ => st.chain.reverse(),
                        }
                        r.st = st.clone();
                        *params.state.write() = st;
                        r.shared.bump_settings();
                        r.shared.bump_waves();

                    }
                }
            }
            provide_heavy(&r.st, &r.shared, &mut prov);
            r.e.sync(&params, &r.shared);
            let chunk = &x[t..t + 256];
            y.extend(r.run(chunk));
            pos += 256;
        }
        let _ = pos;
        // steady: the second before the first event
        let steady = d2(&y[3000..events[0] - 100]).max(1e-4);
        for &e in &events {
            let w = d2(&y[e.saturating_sub(4)..(e + 960).min(y.len())]);
            // the sound may be different after the change: compare with both sides
            let after = d2(&y[(e + 2400).min(y.len() - 1)..(e + 4800).min(y.len())]);
            let before = d2(&y[e.saturating_sub(2400)..e.saturating_sub(4)]);
            let steady = steady.max(after).max(before);
            if w > steady * 4.0 + 0.02 {
                bad.push(format!("{}: change at {e} ({}): {w:.4} vs steady {steady:.4}", kind.name(), (e / 256 / 24) % 8));
            }
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// After the music stops: the tails die away, nothing gets slow (denormals), no DC is left.
#[test]
fn silence_after_music_is_cheap_and_clean() {
    ftz();
    let mut r = Rig::new();
    for kind in [Kind::Filter, Kind::Liquid, Kind::Drive, Kind::Reverb, Kind::Compressor, Kind::Width, Kind::Pan] {
        let s = r.add(kind);
        for l in 0..kind.n_lanes() {
            r.wave(s, l, Wave::flat(0.6));
        }
    }
    r.st.slots[0].f_warm = true;
    r.st.slots[1].l_type = 2;
    r.st.slots[3].r_type = 0;
    r.ready();
    let x = music(48000, 48000.0);
    let t0 = Instant::now();
    let _ = r.run(&x);
    let loud = t0.elapsed().as_secs_f64();
    let z = vec![[0.0f32; 2]; 48000 * 8];
    let t1 = Instant::now();
    let y = r.run(&z);
    let quiet = t1.elapsed().as_secs_f64() / 8.0;
    let tail = peak(&y[48000 * 7..]);
    assert!(quiet < loud * 2.0 + 0.01, "silence costs {quiet:.4} s/s vs music {loud:.4} s/s (denormals?)");
    assert!(tail < 1e-3, "tail still {tail}");
}

/// Tempo 20 .. 999 BPM, the song position jumping around, every length, Hz and pitch at the
/// ends of their ranges: finite and bounded everywhere.
#[test]
fn tempo_position_and_rate_extremes() {
    let mut rng = Rng(5);
    for &kind in Kind::SHAPERS.iter() {
        for (tempo, mode, hz, len) in [(20.0, 0u8, 1.0, 0u8), (999.0, 0, 1.0, 12), (120.0, 1, 0.02, 5), (120.0, 1, 5000.0, 5), (120.0, 2, 1.0, 5)] {
            let mut r = Rig::new();
            let s = r.add(kind);
            r.st.slots[s].t_range = 4;
            for l in 0..kind.n_lanes() {
                r.wave(s, l, Wave::preset(Preset::Sine));
                for b in 0..3 {
                    let lf = &mut r.st.slots[s].lfo[l][b];
                    lf.mode = mode;
                    lf.hz = hz;
                    lf.length = len;
                    lf.transpose = 36.0;
                }
            }
            r.t = Transport { tempo, pos_beats: Some(0.0), playing: true };
            r.ready();
            let x = music(48000, 48000.0);
            let mut y = Vec::new();
            for (i, c) in x.chunks(512).enumerate() {
                if i % 7 == 3 {
                    r.t.pos_beats = Some(rng.uni() as f64 * 64.0);
                }
                r.e.begin_block(&r.ctl, &r.t, &r.shared);
                for (j, v) in c.iter().enumerate() {
                    let note = if (i * 512 + j) % 9000 == 0 { Some([0u8, 127, 60][i % 3]) } else { None };
                    y.push(r.e.process(*v, [0.0; 2], true, note, &r.shared));
                }
                r.e.end_block(&r.shared);
                r.advance(c.len());
            }
            let ok = y.iter().all(|v| v[0].is_finite() && v[1].is_finite());
            let pk = peak(&y);
            assert!(ok && pk < 6.0, "{} tempo {tempo} mode {mode} hz {hz} len {len}: peak {pk}", kind.name());
        }
    }
}

/// No DC from the asymmetric settings.
#[test]
fn no_dc_offset() {
    for (kind, knobs) in [(Kind::Drive, vec![(0usize, 24.0f32), (2, 1.0)]), (Kind::Crush, vec![(0, 20.0), (1, 1.0)]), (Kind::Filter, vec![(1, 1.0)])] {
        let mut r = Rig::new();
        let s = r.add(kind);
        r.st.slots[s].f_warm = true;
        r.wave(s, 0, Wave::flat(0.8));
        for (k, v) in knobs {
            r.knob(s, k, v);
        }
        r.ready();
        let y = r.run(&sine_at(48000 * 2, 100.0, 48000.0, 0.5));
        let mean = y[48000..].iter().map(|v| v[0]).sum::<f32>() / 48000.0;
        assert!(mean.abs() < 0.01, "{}: DC {mean}", kind.name());
    }
}

/// Mono hosts: one channel in, one out.
#[test]
fn mono_is_fine() {
    for &kind in Kind::ALL.iter() {
        let mut r = Rig::new();
        let s = r.add(kind);
        for l in 0..kind.n_lanes() {
            r.wave(s, l, Wave::preset(Preset::Sine));
        }
        r.ready();
        let x = music(24000, 48000.0);
        r.e.begin_block(&r.ctl, &r.t, &r.shared);
        for v in &x {
            let o = r.e.process([v[0], v[0]], [0.0; 2], false, None, &r.shared);
            assert!(o[0].is_finite() && o[0].abs() < 4.0, "{}", kind.name());
        }
        r.e.end_block(&r.shared);
    }
}

/// The Time item reaches as far back as its range says, at the slowest tempo too.
#[test]
fn time_reaches_its_whole_range() {
    let mut r = Rig::new();
    let s = r.add(Kind::Time);
    r.st.slots[s].t_range = 4; // 8 bars
    r.st.slots[s].lfo[0][0].length = 2; // 8 bars
    r.wave(s, 0, Wave::from(&[Pt::new(0.0, 1.0, 0.0), Pt::new(1.0, 0.0, 0.0)]));
    r.t = Transport { tempo: 120.0, pos_beats: Some(0.0), playing: true };
    r.ready();
    // 8 bars at 120 = 16 s: the end of the cycle must reach ~16 s back
    let mut x = vec![[0.0f32; 2]; 48000 * 16];
    x[100] = [1.0, 1.0];
    let y = r.run(&x);
    let hit = y.iter().enumerate().skip(48000).find(|(_, v)| v[0].abs() > 0.05).map(|(i, _)| i);
    assert!(hit.is_some(), "the old click never came back (buffer too short?)");
}

/// Cost of every kind on its own (multiband, envelopes on): `cargo test cost_per_kind -- --ignored --nocapture`
#[test]
#[ignore]
fn cost_per_kind() {
    for &kind in Kind::ALL.iter() {
        for multi in [false, true] {
            let mut r = Rig::new();
            let s = r.add(kind);
            r.st.slots[s].multiband = multi;
            for l in 0..2 {
                for b in 0..3 {
                    r.st.slots[s].env[l][b].on = true;
                    r.st.slots[s].env[l][b].input = 1;
                }
            }
            r.ready();
            let x = music(48000, 48000.0);
            let _ = r.run(&x[..4800]);
            let t = Instant::now();
            let _ = r.run(&x);
            eprintln!("{:<10} {}: {:.2} ms per s", kind.name(), if multi { "MB" } else { "  " }, t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}


/// A 1-shot in sync with a playing song holds at its end marker (no glitch at block starts) and
/// starts again with the next cycle.
#[test]
fn one_shot_in_sync_holds_cleanly() {
    let mut r = Rig::new();
    let v = r.add(Kind::Volume);
    {
        let lfo = &mut r.st.slots[v].lfo[0][0];
        lfo.looping = false;
        lfo.end = 0.5;
        lfo.smooth = 0.0;
        lfo.length = 7; // 1/4 = 24000 samples at 120 BPM
    }
    r.wave(v, 0, Wave::preset(Preset::SawUp));
    r.t = Transport { tempo: 120.0, pos_beats: Some(0.0), playing: true };
    r.ready();
    let y = r.run(&dc(48000, 1.0));
    // 12000 .. 24000: held at 0.5 exactly
    let held = &y[12500..23900];
    let dev = held.iter().map(|s| (s[0] - 0.5).abs()).fold(0.0, f32::max);
    assert!(dev < 0.01, "held part moves by {dev}");
    // the next cycle starts low again
    assert!(y[24100][0] < 0.1, "{}", y[24100][0]);
}
