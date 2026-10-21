//! Speech transcript of the microphone track, generated locally with
//! OpenAI Whisper, and the word grouping used for captions and the
//! transcript editor.

use crate::edit::EditConfig;
use crate::project::Recording;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const FILE: &str = "transcript.json";

/// Pauses at least this long are shown and can be tightened.
pub const LONG_PAUSE: f64 = 1.0;
/// What a tightened pause keeps on each side.
pub const PAUSE_KEEP: f64 = 0.25;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Word {
    /// Video time, seconds.
    pub start: f64,
    pub end: f64,
    /// As spoken, without surrounding spaces.
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub language: String,
    pub model: String,
    pub words: Vec<Word>,
}

impl Transcript {
    pub fn load(dir: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).ok()?).ok()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(FILE), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Indices of filled pauses ("um", "uh", ...).
    pub fn fillers(&self) -> Vec<usize> {
        self.words.iter().enumerate().filter(|(_, w)| is_filler(&w.text)).map(|(i, _)| i).collect()
    }

    /// Long silences between consecutive words: (index of the word after
    /// the pause, start, end).
    pub fn pauses(&self) -> Vec<(usize, f64, f64)> {
        self.words
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[1].start - w[0].end >= LONG_PAUSE)
            .map(|(i, w)| (i + 1, w[0].end, w[1].start))
            .collect()
    }
}

pub fn is_filler(word: &str) -> bool {
    let w: String = word.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    matches!(w.as_str(), "um" | "umm" | "uh" | "uhh" | "uhm" | "erm" | "er" | "ah" | "hmm" | "hm" | "mm" | "mhm")
}

/// Whisper models offered in the Captions tab, with the help text shown
/// under the chips.
pub const MODELS: [(&str, &str, &str); 3] = [
    ("base", "Base", "Fastest. About 140MB, downloaded on first use."),
    ("small", "Small", "Will download 466MB on first use. 2.5 times slower than Base, but more accurate. Produces good results in most cases."),
    ("medium", "Medium", "Will download 1.5GB on first use. Slow, but the most accurate."),
];

/// Languages offered (Whisper code, label). "" lets Whisper detect it.
pub const LANGUAGES: [(&str, &str); 14] = [
    ("", "Auto"),
    ("en", "English"),
    ("ar", "Arabic"),
    ("de", "German"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("zh", "Chinese"),
];

pub fn whisper_available() -> bool {
    which("whisper")
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

#[derive(Default)]
pub struct Progress {
    /// 0..=1000
    pub permille: AtomicU32,
    pub cancel: AtomicBool,
}

/// Transcribes the recording's microphone track. Blocking; run it on a
/// worker thread.
pub fn generate(rec: &Recording, model: &str, language: &str, prompt: &str, progress: &Progress) -> Result<Transcript> {
    let Some(track) = rec.audio.iter().find(|a| a.kind == "microphone") else { bail!("this recording has no microphone audio") };
    let audio = rec.dir.join(&track.file);
    let out_dir = rec.dir.join(".whisper");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)?;
    // Whisper tends to drop filled pauses; a prompt containing them keeps
    // them in, so they can be cut later.
    let mut initial = String::from("Umm, let me think like, hmm... Okay, here's what I'm, uh, thinking.");
    if !prompt.trim().is_empty() {
        initial = format!("{initial} {}", prompt.trim());
    }
    let mut cmd = Command::new("whisper");
    cmd.arg(&audio)
        .args(["--model", model, "--word_timestamps", "True", "--output_format", "json", "--fp16", "False", "--verbose", "True"])
        .arg("--output_dir")
        .arg(&out_dir)
        .args(["--initial_prompt", &initial]);
    if !language.is_empty() {
        cmd.args(["--language", language]);
    }
    let mut child = cmd
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running whisper")?;
    let stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::BufReader::new(stderr), &mut s);
        s
    });
    let audio_len = (rec.duration - track.offset).max(0.1);
    for line in std::io::BufReader::new(child.stdout.take().unwrap()).lines() {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&out_dir);
            bail!("cancelled");
        }
        // "[00:12.340 --> 00:15.000]  text"
        let Ok(line) = line else { break };
        if let Some(end) = line.split("-->").nth(1).and_then(|r| r.split(']').next()) {
            if let Some(t) = parse_ts(end.trim()) {
                progress.permille.store(((t / audio_len).clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
            }
        }
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        bail!("whisper failed: {last}");
    }
    let stem = audio.file_stem().unwrap().to_string_lossy().to_string();
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out_dir.join(format!("{stem}.json")))?)?;
    let _ = std::fs::remove_dir_all(&out_dir);
    let mut words = vec![];
    for seg in json["segments"].as_array().into_iter().flatten() {
        for w in seg["words"].as_array().into_iter().flatten() {
            let text = w["word"].as_str().unwrap_or("").trim().to_string();
            let (Some(s), Some(e)) = (w["start"].as_f64(), w["end"].as_f64()) else { continue };
            if text.is_empty() {
                continue;
            }
            words.push(Word { start: s + track.offset, end: e + track.offset, text });
        }
    }
    progress.permille.store(1000, Ordering::Relaxed);
    Ok(Transcript { language: json["language"].as_str().unwrap_or(language).to_string(), model: model.to_string(), words })
}

/// "mm:ss.mmm" or "hh:mm:ss.mmm" to seconds.
fn parse_ts(s: &str) -> Option<f64> {
    let mut t = 0.0;
    for part in s.split(':') {
        t = t * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(t)
}

/// One caption: a run of words shown together.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    /// Indices into the transcript's words.
    pub words: Vec<usize>,
}

/// Groups the visible words into caption lines: break at long pauses, after
/// sentence ends, and at about `max_chars` characters.
pub fn lines(t: &Transcript, edit: &EditConfig, max_chars: usize) -> Vec<Line> {
    let mut out: Vec<Line> = vec![];
    let mut cur: Option<Line> = None;
    let mut chars = 0;
    let mut prev_end = f64::NEG_INFINITY;
    let mut prev_text = String::new();
    for (i, w) in t.words.iter().enumerate() {
        if edit.captions.hidden_words.contains(&i) || edit.is_cut(w.start, w.end) {
            continue;
        }
        let text = edit.captions.word_text(i, &w.text);
        let sentence_end = prev_text.ends_with(['.', '?', '!']);
        let brk = cur.is_some() && (w.start - prev_end > 0.8 || chars + text.len() + 1 > max_chars || (sentence_end && chars > 12));
        if brk {
            out.extend(cur.take());
            chars = 0;
        }
        let line = cur.get_or_insert(Line { start: w.start, end: w.end, words: vec![] });
        line.words.push(i);
        line.end = w.end;
        chars += text.len() + 1;
        prev_end = w.end;
        prev_text = text.to_string();
    }
    out.extend(cur);
    out
}

/// SubRip text of the caption lines, in output time (cuts removed).
pub fn srt(t: &Transcript, edit: &EditConfig, duration: f64) -> String {
    let fmt = |s: f64| {
        let ms = (s.max(0.0) * 1000.0).round() as u64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    };
    let mut out = String::new();
    for (n, l) in lines(t, edit, 42).iter().enumerate() {
        let (Some(a), Some(b)) = (edit.output_time(l.start, duration), edit.output_time(l.end, duration)) else { continue };
        let text: Vec<&str> = l.words.iter().map(|i| edit.captions.word_text(*i, &t.words[*i].text)).collect();
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", n + 1, fmt(a), fmt(b), text.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: f64, end: f64, text: &str) -> Word {
        Word { start, end, text: text.into() }
    }

    #[test]
    fn fillers_pauses_lines() {
        let t = Transcript {
            words: vec![w(0.0, 0.3, "Hey,"), w(0.35, 0.5, "um,"), w(0.6, 0.9, "today."), w(2.5, 2.8, "Next"), w(2.85, 3.0, "line.")],
            ..Default::default()
        };
        assert_eq!(t.fillers(), vec![1]);
        assert_eq!(t.pauses().len(), 1);
        let edit = EditConfig::default();
        let l = lines(&t, &edit, 42);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].words, vec![3, 4]);
        let mut edit = EditConfig::default();
        edit.cuts.push(crate::edit::Cut { start: 0.32, end: 0.55 });
        assert_eq!(lines(&t, &edit, 42)[0].words, vec![0, 2]);
        let s = srt(&t, &edit, 3.0);
        assert!(s.contains("00:00:02,270 --> 00:00:02,770\nNext line."), "{s}");
    }
}
