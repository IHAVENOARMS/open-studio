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
        // Arabic hesitations.
        || matches!(w.as_str(), "امم" | "اممم" | "ام" | "ممم" | "اه" | "آه" | "إمم" | "ااه")
}

/// Words that are often padding ("so", "like", Egyptian "يعني") but just as
/// often meaningful: marked in the transcript, never removed automatically.
pub fn is_soft_filler(word: &str) -> bool {
    let w: String = word.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    matches!(w.as_str(), "so" | "like" | "basically" | "actually" | "literally" | "okay" | "يعني" | "طيب" | "بقى")
}

/// Whisper models offered in the Captions tab, with the help text shown
/// under the chips.
pub const MODELS: [(&str, &str, &str); 4] = [
    ("base", "Base", "Will download 148MB on first use. Fastest, good for clear English."),
    ("small", "Small", "Will download 488MB on first use. 2.5 times slower than Base, but more accurate. Produces good results in most cases."),
    ("medium", "Medium", "Will download 1.5GB on first use. 8 times slower than Base, but more accurate. Good for other languages and accents."),
    ("large-v3-turbo", "Large", "Will download 1.6GB on first use. The slowest, around twice the recording length on a laptop CPU, but the most accurate. Best for Arabic dialects, mixed languages and strong accents."),
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
    whisper_cpp().is_some() || which("whisper")
}

/// whisper.cpp's command-line program (much faster than Python Whisper on
/// the CPU): bundled next to our binary, or installed (`whisper-cpp`).
fn whisper_cpp() -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    if let Some(d) = std::env::var_os("APPDIR") {
        dirs.push(std::path::PathBuf::from(d).join("usr/bin"));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(d);
    }
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    for d in dirs {
        for name in ["whisper-cli", "whisper-cpp"] {
            if d.join(name).is_file() {
                return Some(d.join(name));
            }
        }
    }
    None
}

/// The ggml model file for whisper.cpp, downloaded on first use; download
/// progress goes to `progress` (0..1000) as it arrives.
fn ggml_model(model: &str, progress: &Progress) -> Result<std::path::PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.data_dir().join("models"))
        .unwrap_or_else(|| std::path::PathBuf::from(".open-studio-models"));
    let path = dir.join(format!("ggml-{model}.bin"));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin");
    let total = Command::new("curl")
        .args(["-fsIL", &url])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .rev()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<u64>().ok()))
        })
        .unwrap_or(0);
    let tmp = path.with_extension("part");
    let mut child = Command::new("curl").args(["-fsSL", "-o"]).arg(&tmp).arg(&url).spawn().context("running curl")?;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = std::fs::remove_file(&tmp);
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let _ = std::fs::remove_file(&tmp);
                bail!("could not download the {model} model");
            }
            break;
        }
        if total > 0 {
            let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
            // Downloading is the first 40% of the bar.
            progress.permille.store((got * 400 / total).min(400) as u32, Ordering::Relaxed);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Transcribes with whisper.cpp: one segment per word (`-ml 1 -sow`).
fn generate_cpp(cli: &std::path::Path, audio: &std::path::Path, offset: f64, model: &str, language: &str, prompt: &str, progress: &Progress, dir: &std::path::Path) -> Result<Transcript> {
    let downloaded = !ggml_model_exists(model);
    let model_path = ggml_model(model, progress)?;
    let base = if downloaded { 400 } else { 0 };
    let wav = dir.join("audio16k.wav");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(audio)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .status()?
        .success();
    if !ok {
        bail!("could not read the microphone audio");
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let out = dir.join("words");
    let mut cmd = Command::new(cli);
    cmd.arg("-m").arg(&model_path).arg("-f").arg(&wav).arg("-of").arg(&out);
    cmd.args(["-l", if language.is_empty() { "auto" } else { language }]);
    cmd.args(["-ml", "1", "-sow", "-ojf", "-pp", "-t", &threads.to_string()]);
    if !prompt.is_empty() {
        cmd.args(["--prompt", prompt]);
    }
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("running whisper-cli")?;
    let stderr = child.stderr.take().unwrap();
    let mut last_err = String::new();
    for line in std::io::BufReader::new(stderr).lines() {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("cancelled");
        }
        let Ok(line) = line else { break };
        // "whisper_print_progress_callback: progress =  45%"
        if let Some(p) = line.split("progress =").nth(1).and_then(|v| v.trim().trim_end_matches('%').parse::<u32>().ok()) {
            progress.permille.store(base + p.min(100) * (1000 - base) / 100, Ordering::Relaxed);
        } else if line.to_lowercase().contains("error") {
            last_err = line;
        }
    }
    if !child.wait()?.success() {
        bail!("whisper-cli failed: {last_err}");
    }
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.with_extension("json"))?)?;
    let mut words = vec![];
    for seg in json["transcription"].as_array().into_iter().flatten() {
        let text = seg["text"].as_str().unwrap_or("").trim().to_string();
        let (Some(a), Some(b)) = (seg["offsets"]["from"].as_f64(), seg["offsets"]["to"].as_f64()) else { continue };
        // Skip empties and non-speech markers like "[MUSIC]".
        if text.is_empty() || (text.starts_with('[') && text.ends_with(']')) || (text.starts_with('(') && text.ends_with(')')) {
            continue;
        }
        words.push(Word { start: a / 1000.0 + offset, end: b / 1000.0 + offset, text });
    }
    let lang = json["result"]["language"].as_str().unwrap_or(language).to_string();
    Ok(Transcript { language: lang, model: model.to_string(), words })
}

fn ggml_model_exists(model: &str) -> bool {
    directories::ProjectDirs::from("", "", "open-studio").is_some_and(|d| d.data_dir().join("models").join(format!("ggml-{model}.bin")).is_file())
}

/// Drops words that fall where the microphone was silent: Whisper invents
/// text for silence and noise.
fn keep_spoken(rec: &Recording, mut t: Transcript) -> Result<Transcript> {
    let speech = crate::voice::speech_spans(rec);
    if speech.is_empty() {
        bail!("no speech found in the microphone audio");
    }
    let pad = 0.4;
    t.words.retain(|w| speech.iter().any(|(a, b)| w.end > a - pad && w.start < b + pad));
    if t.words.is_empty() {
        bail!("no speech found in the microphone audio");
    }
    Ok(t)
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
    if let Some(cli) = whisper_cpp() {
        // The English filler prompt confuses other languages; only use it
        // when the language is English or the user wrote one.
        let prompt = match (language, prompt.trim()) {
            (_, p) if !p.is_empty() => p.to_string(),
            ("en", _) => "Umm, let me think like, hmm... Okay, here's what I'm, uh, thinking.".to_string(),
            _ => String::new(),
        };
        let r = generate_cpp(&cli, &audio, track.offset, model, language, &prompt, progress, &out_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
        let t = keep_spoken(rec, r?)?;
        progress.permille.store(1000, Ordering::Relaxed);
        return Ok(t);
    }
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
    keep_spoken(rec, Transcript { language: json["language"].as_str().unwrap_or(language).to_string(), model: model.to_string(), words })
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
    /// Right to left: the sentence the line starts in begins with Arabic.
    pub rtl: bool,
}

/// Groups the visible words into caption lines: break at long pauses, after
/// sentence ends, and at about `max_chars` characters. A line's direction
/// is its sentence's, set by the sentence's first letter, so a line inside an
/// Arabic sentence that happens to start with an English word stays right to
/// left; a sentence in the other direction always starts a new line.
pub fn lines(t: &Transcript, edit: &EditConfig, max_chars: usize) -> Vec<Line> {
    // Visible words, split into sentences at sentence ends and long pauses.
    let mut sentences: Vec<Vec<(usize, &str)>> = vec![];
    let mut prev: Option<(f64, &str)> = None;
    for (i, w) in t.words.iter().enumerate() {
        if edit.captions.hidden_words.contains(&i) || edit.is_cut(w.start, w.end) {
            continue;
        }
        let text = edit.captions.word_text(i, &w.text);
        let new = prev.is_none_or(|(end, p)| w.start - end > 0.8 || p.ends_with(['.', '?', '!', '؟']));
        if new {
            sentences.push(vec![]);
        }
        sentences.last_mut().unwrap().push((i, text));
        prev = Some((w.end, text));
    }
    let mut out: Vec<Line> = vec![];
    let mut cur: Option<Line> = None;
    let mut chars = 0;
    for sentence in sentences {
        let rtl = crate::bidi::paragraph_rtl(sentence.iter().map(|(_, s)| *s));
        let (start, _) = sentence[0];
        let gap = cur.as_ref().map_or(0.0, |l| t.words[start].start - l.end);
        // Short sentences share a line, unless they run in different directions.
        if cur.as_ref().is_some_and(|l| gap > 0.8 || chars > 12 || l.rtl != rtl) {
            out.extend(cur.take());
            chars = 0;
        }
        for (i, text) in sentence {
            let len = text.chars().count();
            if cur.is_some() && chars + len + 1 > max_chars {
                out.extend(cur.take());
                chars = 0;
            }
            let w = &t.words[i];
            let line = cur.get_or_insert(Line { start: w.start, end: w.end, words: vec![], rtl });
            line.words.push(i);
            line.end = w.end;
            chars += len + 1;
        }
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
    fn line_direction_follows_the_sentence_start() {
        let text = "يعني the new feature is really useful for recording tutorials and demos. OK this one is English.";
        let words = text.split(' ').enumerate().map(|(i, t)| w(i as f64 * 0.3, i as f64 * 0.3 + 0.25, t)).collect();
        let t = Transcript { language: "ar".into(), model: "base".into(), words };
        let lines = lines(&t, &EditConfig::default(), 24);
        assert!(lines.len() >= 3);
        // The Arabic sentence's later lines start with English but stay right to left.
        let has = |l: &Line, word: &str| l.words.iter().any(|i| t.words[*i].text == word);
        let end = lines.iter().position(|l| has(l, "demos.")).unwrap();
        assert!(lines[..=end].iter().all(|l| l.rtl));
        assert!(lines[1..=end].iter().any(|l| !crate::bidi::is_rtl(&t.words[l.words[0]].text)));
        assert!(!lines.last().unwrap().rtl);
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
