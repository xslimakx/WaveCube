//! User noises and impulse responses: WAV files in `<data dir>/WaveCube/Noises` and
//! `<data dir>/WaveCube/IRs` (one level of subfolders). Loaded on a non-audio thread.

use crate::{Shared, State};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct Sample {
    pub data: Vec<[f32; 2]>,
    pub sr: f32,
}

pub fn root() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("WaveCube"))
}

pub fn noise_dir() -> Option<PathBuf> {
    root().map(|d| d.join("Noises"))
}

pub fn ir_dir() -> Option<PathBuf> {
    root().map(|d| d.join("IRs"))
}

/// WAV files in a folder and its direct subfolders, as names relative to the folder.
pub fn list(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut scan = |d: &Path, prefix: &str| {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false) {
                    if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                        out.push(format!("{prefix}{n}"));
                    }
                }
            }
        }
    };
    scan(dir, "");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    scan(&p, &format!("{n}/"));
                }
            }
        }
    }
    out.sort_by_key(|s| s.to_lowercase());
    out.truncate(1000);
    out
}

/// Reads a WAV file as stereo f32 (mono is copied to both channels). At most `max_s` seconds.
pub fn load_wav(path: &Path, max_s: f32) -> Option<Sample> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 100 * 1024 * 1024 {
        return None;
    }
    let mut r = hound::WavReader::open(path).ok()?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let max = (max_s * spec.sample_rate as f32) as usize * ch;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().take(max).filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            r.samples::<i32>().take(max).filter_map(|s| s.ok()).map(|s| s as f32 * scale).collect()
        }
    };
    let data: Vec<[f32; 2]> = raw.chunks(ch).map(|c| [c[0], *c.get(1).unwrap_or(&c[0])]).collect();
    if data.is_empty() {
        return None;
    }
    Some(Sample { data, sr: spec.sample_rate as f32 })
}

/// Linear resampling to `sr` (for impulse responses).
pub fn resample(s: &Sample, sr: f32) -> Vec<[f32; 2]> {
    if (s.sr - sr).abs() < 0.5 {
        return s.data.clone();
    }
    let ratio = s.sr / sr;
    let n = (s.data.len() as f32 / ratio) as usize;
    (0..n)
        .map(|i| {
            let p = i as f32 * ratio;
            let k = p.floor() as usize;
            let f = p - k as f32;
            let a = s.data[k.min(s.data.len() - 1)];
            let b = s.data[(k + 1).min(s.data.len() - 1)];
            [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
        })
        .collect()
}

/// Loads every user noise / IR the state refers to into `shared` (not on the audio thread).
pub fn load_user_media(st: &State, shared: &Shared) {
    if let Some(d) = noise_dir() {
        let mut v = shared.noises.lock();
        v.clear();
        for n in &st.user_noises {
            let s = load_wav(&d.join(n), 30.0).unwrap_or(Sample { data: vec![[0.0; 2]; 16], sr: 48000.0 });
            v.push(Arc::new(s));
        }
    }
    if let Some(d) = ir_dir() {
        let mut v = shared.irs.lock();
        v.clear();
        for n in &st.user_irs {
            let s = load_wav(&d.join(n), 6.0).unwrap_or(Sample { data: vec![[0.0; 2]; 16], sr: 48000.0 });
            v.push(Arc::new(s));
        }
    }
    shared.media_version.fetch_add(1, Ordering::AcqRel);
}

/// Index of a user file in the state's list (adds it if new).
pub fn user_index(list: &mut Vec<String>, name: &str) -> usize {
    if let Some(i) = list.iter().position(|n| n == name) {
        i
    } else {
        list.push(name.to_string());
        list.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip_and_resample() {
        let dir = std::env::temp_dir().join(format!("wc_media_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let path = dir.join("sub").join("t.wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..4410 {
            w.write_sample(((i as f32 * 0.1).sin() * 16000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        assert_eq!(list(&dir), vec!["sub/t.wav".to_string()]);
        let s = load_wav(&path, 10.0).unwrap();
        assert_eq!(s.data.len(), 4410);
        assert!((s.data[10][0] - (1.0f32).sin() * 16000.0 / 32768.0).abs() < 1e-3);
        assert_eq!(s.data[10][0], s.data[10][1]);
        let r = resample(&s, 48000.0);
        assert!((r.len() as i64 - 4800).abs() <= 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
