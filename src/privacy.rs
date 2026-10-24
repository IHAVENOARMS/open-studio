//! Hide sensitive data and Hide faces, as in Screen Studio 4.1: emails,
//! phone and card numbers, keys and custom phrases on screen get covers
//! that blend into the video, and faces (avatars, photos, people in calls)
//! get blurred. Covers follow what they hide while the page scrolls.
//!
//! The screen video is looked at a few times a second. Frames that changed
//! are read with tesseract (OCR) and searched for faces with rustface; what
//! was found is cached in privacy.json. Matching and following happen when
//! the covers are built, so changing the phrases needs no new pass.

use crate::project::Recording;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub const FILE: &str = "privacy.json";
/// Looks per second.
pub const RATE: f64 = 4.0;
/// Covers start and end this much around the first and last look that saw
/// their text, so nothing shows between looks.
const PAD: f64 = 0.6 / RATE;

/// A word read on screen. `rect` is x, y, w, h as fractions of the frame.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// Words with the same line id are one line of text.
    pub line: u32,
    pub rect: [f64; 4],
}

/// What one look found; None means the frame hadn't changed since the look
/// before, so the same.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Sample<T> {
    pub t: f64,
    #[serde(default)]
    pub found: Option<T>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Analysis {
    pub text: Option<Vec<Sample<Vec<Word>>>>,
    pub faces: Option<Vec<Sample<Vec<[f64; 4]>>>>,
}

impl Analysis {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(FILE), serde_json::to_string(self)?)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoverKind {
    /// Sensitive text: filled with the colour around it.
    Text,
    /// A face: blurred.
    Face,
}

/// One thing hidden over time: its rect at each look that saw it.
#[derive(Clone, Debug)]
pub struct Cover {
    pub kind: CoverKind,
    pub keys: Vec<(f64, [f64; 4])>,
}

impl Cover {
    /// Where the cover is at `t`, if it's showing then.
    pub fn at(&self, t: f64) -> Option<[f64; 4]> {
        let (first, last) = (self.keys.first()?, self.keys.last()?);
        if t < first.0 - PAD || t > last.0 + PAD {
            return None;
        }
        let i = self.keys.partition_point(|k| k.0 <= t);
        Some(match (i.checked_sub(1).map(|j| self.keys[j]), self.keys.get(i)) {
            (Some(a), Some(b)) => {
                let f = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
                std::array::from_fn(|k| a.1[k] + (b.1[k] - a.1[k]) * f)
            }
            (Some(a), None) => a.1,
            (None, Some(b)) => b.1,
            (None, None) => return None,
        })
    }
}

// ---- Finding sensitive text ------------------------------------------------

/// Kinds of sensitive text, for labels and tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sensitive {
    Email,
    Number,
    Key,
    Phrase,
}

/// Character ranges of sensitive text in `line`.
pub fn find_sensitive(line: &str, phrases: &[String]) -> Vec<(usize, usize, Sensitive)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = vec![];
    // Emails: something@something. OCR often drops or garbles the dot, so
    // the domain only needs a few characters.
    let local = |c: char| c.is_alphanumeric() || "._%+-".contains(c);
    let domain = |c: char| c.is_alphanumeric() || ".-".contains(c);
    for (i, &c) in chars.iter().enumerate() {
        if c != '@' {
            continue;
        }
        let mut a = i;
        while a > 0 && local(chars[a - 1]) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < chars.len() && domain(chars[b]) {
            b += 1;
        }
        if i - a >= 1 && b - i > 3 {
            out.push((a, b, Sensitive::Email));
        }
    }
    // Phone and card numbers: 9 to 19 digits, possibly grouped with spaces,
    // dashes, dots or brackets, maybe after a +.
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() || ((chars[i] == '+' || chars[i] == '(') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())) {
            let start = i;
            let (mut digits, mut end, mut j) = (0, i, i);
            while j < chars.len() {
                let c = chars[j];
                if c.is_ascii_digit() {
                    digits += 1;
                    end = j + 1;
                } else if !(" -.()+".contains(c)) {
                    break;
                }
                j += 1;
            }
            // A number glued to letters is an id or a word, not a phone.
            let glued = start > 0 && chars[start - 1].is_alphabetic() || chars.get(end).is_some_and(|c| c.is_alphabetic());
            let run: String = chars[start..end].iter().collect();
            if (9..=19).contains(&digits) && !glued && !starts_with_date(&run) {
                out.push((start, end, Sensitive::Number));
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    // Keys and tokens: long runs mixing letters and digits, or known key
    // prefixes. Links and file paths aren't keys.
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let tok: String = chars[start..i].iter().collect();
        let t = tok.trim_matches(|c: char| "\"'`,;()[]{}<>".contains(c));
        let known = ["sk-", "sk_", "pk_", "rk_", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza", "eyJ", "glpat-"]
            .iter()
            .any(|p| t.starts_with(p));
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
        let mixed = letters > 0 && digits >= 2 && t.chars().any(|c| c.is_uppercase()) && t.chars().any(|c| c.is_lowercase());
        let hexish = t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit()) && digits > 0 && letters > 0;
        let link = t.contains("://") || t.starts_with("www.") || t.matches('/').count() >= 2;
        if !link && ((known && t.len() >= 16) || (t.chars().count() >= 20 && mixed) || hexish) {
            out.push((start, i, Sensitive::Key));
        }
    }
    // Custom phrases, ignoring case.
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    if lower.len() == chars.len() {
        for p in phrases.iter().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()) {
            let p: Vec<char> = p.chars().collect();
            let mut i = 0;
            while i + p.len() <= lower.len() {
                if lower[i..i + p.len()] == p[..] {
                    out.push((i, i + p.len(), Sensitive::Phrase));
                    i += p.len();
                } else {
                    i += 1;
                }
            }
        }
    }
    out
}

/// "2026-10-24 …", "24/10/2026 …": a date (maybe with a time), not a phone.
fn starts_with_date(s: &str) -> bool {
    let groups: Vec<&str> = s.split(|c: char| "-/.".contains(c)).take(3).collect();
    if groups.len() < 3 {
        return false;
    }
    let len = |g: &str| g.chars().take_while(|c| c.is_ascii_digit()).count();
    let (a, b, c) = (len(groups[0]), len(groups[1]), len(groups[2]));
    let ymd = a == 4 && (1..=2).contains(&b) && (1..=2).contains(&c);
    let dmy = (1..=2).contains(&a) && (1..=2).contains(&b) && (c == 2 || c == 4);
    (ymd || dmy) && groups[..2].iter().all(|g| g.chars().all(|c| c.is_ascii_digit()))
}

/// Sensitive text among `words` (one look): what it says (to follow it to
/// the next look) and the rect covering it.
pub fn sensitive_rects(words: &[Word], phrases: &[String], frame: (f64, f64)) -> Vec<(String, [f64; 4])> {
    let mut lines: Vec<u32> = words.iter().map(|w| w.line).collect();
    lines.sort_unstable();
    lines.dedup();
    let mut out = vec![];
    for line in lines {
        let ws: Vec<&Word> = words.iter().filter(|w| w.line == line).collect();
        // Line text, and where each word sits in it.
        let mut text = String::new();
        let mut spans = vec![];
        for w in &ws {
            if !text.is_empty() {
                text.push(' ');
            }
            let a = text.chars().count();
            text.push_str(&w.text);
            spans.push((a, text.chars().count()));
        }
        for (a, b, _) in find_sensitive(&text, phrases) {
            let hit: Vec<&Word> = ws.iter().zip(&spans).filter(|(_, s)| s.0 < b && s.1 > a).map(|(w, _)| *w).collect();
            if hit.is_empty() {
                continue;
            }
            let x0 = hit.iter().map(|w| w.rect[0]).fold(f64::MAX, f64::min);
            let y0 = hit.iter().map(|w| w.rect[1]).fold(f64::MAX, f64::min);
            let x1 = hit.iter().map(|w| w.rect[0] + w.rect[2]).fold(f64::MIN, f64::max);
            let y1 = hit.iter().map(|w| w.rect[1] + w.rect[3]).fold(f64::MIN, f64::max);
            // Some room around the letters, in pixels relative to their height.
            let hpx = (y1 - y0) * frame.1;
            let (px, py) = (0.35 * hpx / frame.0, 0.3 * hpx / frame.1);
            let label: String = text.chars().skip(a).take(b - a).filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            out.push((label, [x0 - px, y0 - py, x1 - x0 + 2.0 * px, y1 - y0 + 2.0 * py]));
        }
    }
    out
}

/// Joins what each look found into covers: the same text (or a face) near
/// where it was a look earlier is the same thing, moved.
fn follow(samples: Vec<(f64, Vec<(String, [f64; 4])>)>, kind: CoverKind) -> Vec<Cover> {
    let mut done: Vec<Cover> = vec![];
    // Covers seen in the previous look, with their labels.
    let mut open: Vec<(String, Cover)> = vec![];
    for (t, items) in samples {
        let mut next: Vec<(String, Cover)> = vec![];
        for (label, r) in items {
            let c = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
            let reach = match kind {
                CoverKind::Text => 0.35,
                CoverKind::Face => r[2].max(r[3]) * 0.8 + 0.03,
            };
            let near = open
                .iter()
                .enumerate()
                .filter(|(_, (l, _))| *l == label)
                .map(|(i, (_, cv))| {
                    let p = cv.keys.last().unwrap().1;
                    let d = ((p[0] + p[2] / 2.0 - c.0).powi(2) + (p[1] + p[3] / 2.0 - c.1).powi(2)).sqrt();
                    (i, d)
                })
                .filter(|(_, d)| *d < reach)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match near {
                Some((i, _)) => {
                    let (l, mut cv) = open.swap_remove(i);
                    cv.keys.push((t, r));
                    next.push((l, cv));
                }
                None => next.push((label, Cover { kind, keys: vec![(t, r)] })),
            }
        }
        done.extend(open.into_iter().map(|(_, c)| c));
        open = next;
    }
    done.extend(open.into_iter().map(|(_, c)| c));
    done
}

/// Fills in looks that found the same as the one before.
fn expand<T: Clone>(samples: &[Sample<T>]) -> Vec<(f64, T)> {
    let mut out: Vec<(f64, T)> = vec![];
    for s in samples {
        match &s.found {
            Some(v) => out.push((s.t, v.clone())),
            None => {
                if let Some(prev) = out.last().map(|p| p.1.clone()) {
                    out.push((s.t, prev));
                }
            }
        }
    }
    out
}

/// Covers for everything sensitive the analysis read.
pub fn text_covers(an: &Analysis, phrases: &[String], frame: (f64, f64)) -> Vec<Cover> {
    let Some(text) = &an.text else { return vec![] };
    let samples = expand(text).into_iter().map(|(t, words)| (t, sensitive_rects(&words, phrases, frame))).collect();
    follow(samples, CoverKind::Text)
}

/// Covers for every face the analysis saw, a bit larger than the face.
pub fn face_covers(an: &Analysis) -> Vec<Cover> {
    let Some(faces) = &an.faces else { return vec![] };
    let samples = expand(faces)
        .into_iter()
        .map(|(t, fs)| (t, fs.into_iter().map(|r| (String::new(), [r[0] - r[2] * 0.2, r[1] - r[3] * 0.25, r[2] * 1.4, r[3] * 1.45])).collect()))
        .collect();
    follow(samples, CoverKind::Face)
}

/// Covers for a project with these settings, cached between rebuilds.
pub fn covers(rec: &Recording, settings: &crate::edit::PrivacySettings) -> Arc<Vec<Cover>> {
    static CACHE: Mutex<Option<(String, Arc<Vec<Cover>>)>> = Mutex::new(None);
    if !settings.hide_sensitive && !settings.hide_faces {
        return Arc::default();
    }
    let path = rec.dir.join(FILE);
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let key = format!("{}|{stamp:?}|{}|{}|{:?}", path.display(), settings.hide_sensitive, settings.hide_faces, settings.phrases);
    if let Some((k, c)) = CACHE.lock().unwrap().as_ref() {
        if *k == key {
            return c.clone();
        }
    }
    let an = Analysis::load(&rec.dir);
    let mut all = vec![];
    if settings.hide_sensitive {
        all.extend(text_covers(&an, &settings.phrases, (rec.width as f64, rec.height as f64)));
    }
    if settings.hide_faces {
        all.extend(face_covers(&an));
    }
    let all = Arc::new(all);
    *CACHE.lock().unwrap() = Some((key, all.clone()));
    all
}

/// Makes sure what these settings hide has been looked for, before an
/// export: exporting without the covers would show what should be hidden.
pub fn ensure(rec: &Recording, settings: &crate::edit::PrivacySettings) -> Result<()> {
    if settings.hide_sensitive || settings.hide_faces {
        analyze(rec, settings.hide_sensitive, settings.hide_faces, &AtomicU32::new(0))?;
    }
    Ok(())
}

// ---- Looking at the video --------------------------------------------------

/// True when tesseract (with English data) can be run.
pub fn ocr_available() -> bool {
    Command::new("tesseract").arg("--list-langs").output().is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == "eng"))
}

/// Words in a greyscale frame, by tesseract.
fn ocr(gray: &[u8], w: u32, h: u32) -> Result<Vec<Word>> {
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "-l", "eng", "--psm", "11", "tsv"])
        // Many frames run side by side; one thread each.
        .env("OMP_THREAD_LIMIT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running tesseract")?;
    let mut stdin = child.stdin.take().unwrap();
    let header = format!("P5\n{w} {h}\n255\n");
    let data = gray.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(header.as_bytes()).and_then(|_| stdin.write_all(&data));
    });
    let mut tsv = String::new();
    child.stdout.take().unwrap().read_to_string(&mut tsv)?;
    let _ = writer.join();
    child.wait()?;
    Ok(parse_tsv(&tsv, w as f64, h as f64))
}

/// Words from tesseract's TSV output, with rects as fractions of the frame.
fn parse_tsv(tsv: &str, w: f64, h: f64) -> Vec<Word> {
    let mut out = vec![];
    for row in tsv.lines().skip(1) {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let text = c[11].trim();
        let conf: f64 = c[10].parse().unwrap_or(-1.0);
        if text.is_empty() || conf < 0.0 {
            continue;
        }
        let n = |i: usize| c[i].parse::<f64>().unwrap_or(0.0);
        let line = (n(2) as u32) * 1_000_000 + (n(3) as u32) * 1000 + n(4) as u32;
        out.push(Word { text: text.to_string(), line, rect: [n(6) / w, n(7) / h, n(8) / w, n(9) / h] });
    }
    out
}

/// Faces in a greyscale frame.
fn faces(det: &mut dyn rustface::Detector, gray: &[u8], w: u32, h: u32) -> Vec<[f64; 4]> {
    let img = rustface::ImageData::new(gray, w, h);
    det.detect(&img)
        .into_iter()
        .map(|f| {
            let b = f.bbox();
            [b.x() as f64 / w as f64, b.y() as f64 / h as f64, b.width() as f64 / w as f64, b.height() as f64 / h as f64]
        })
        .collect()
}

/// Halves a greyscale image until it's at most `max_h` tall.
fn shrink(mut gray: Vec<u8>, mut w: u32, mut h: u32, max_h: u32) -> (Vec<u8>, u32, u32) {
    while h > max_h {
        let (nw, nh) = (w / 2, h / 2);
        let mut out = vec![0u8; (nw * nh) as usize];
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                let i = |dx: usize, dy: usize| gray[(2 * y + dy) * w as usize + 2 * x + dx] as u32;
                out[y * nw as usize + x] = ((i(0, 0) + i(1, 0) + i(0, 1) + i(1, 1)) / 4) as u8;
            }
        }
        (gray, w, h) = (out, nw, nh);
    }
    (gray, w, h)
}

/// A tiny copy of the frame to tell whether anything changed.
fn thumb(gray: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (tw, th) = (w / 8, h / 8);
    let mut out = vec![0u8; (tw * th) as usize];
    for ty in 0..th as usize {
        for tx in 0..tw as usize {
            let mut s = 0u32;
            for dy in 0..8 {
                for dx in 0..8 {
                    s += gray[(ty * 8 + dy) * w as usize + tx * 8 + dx] as u32;
                }
            }
            out[ty * tw as usize + tx] = (s / 64) as u8;
        }
    }
    out
}

fn changed(a: &[u8], b: &[u8]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x.abs_diff(*y) > 6)
}

/// Looks through the screen video for what's asked (text, faces) that the
/// cached analysis doesn't have yet, and saves it. Blocking; `progress` goes
/// 0..1000.
pub fn analyze(rec: &Recording, want_text: bool, want_faces: bool, progress: &AtomicU32) -> Result<Analysis> {
    // One pass at a time: an export waits for the editor's pass (then finds
    // it cached) instead of doing the same work alongside.
    static RUNNING: Mutex<()> = Mutex::new(());
    let _one = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let mut an = Analysis::load(&rec.dir);
    let text = want_text && an.text.is_none();
    let face = want_faces && an.faces.is_none();
    if !text && !face {
        return Ok(an);
    }
    if text && !ocr_available() {
        return Err(anyhow!("install tesseract (with English data) to find sensitive data"));
    }
    let model = if face { Some(crate::face::ensure_model()?) } else { None };
    // Small text reads better enlarged: about 2160 lines tall for OCR.
    let scale = if text { (2160.0 / rec.height as f64).clamp(1.0, 2.0) } else { (1080.0 / rec.height as f64).min(1.0) };
    let (w, h) = (((rec.width as f64 * scale) as u32) & !7, ((rec.height as f64 * scale) as u32) & !7);
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(rec.dir.join(&rec.video))
        .args(["-vf", &format!("fps={RATE},scale={w}:{h}:flags=lanczos,format=gray"), "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running ffmpeg")?;
    let mut stdout = child.stdout.take().unwrap();

    // Workers read changed frames side by side.
    type Job = (usize, Arc<Vec<u8>>);
    type Done = (usize, Option<Vec<Word>>, Option<Vec<[f64; 4]>>);
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).max(1));
    let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<Job>(workers * 2);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    let mut handles = vec![];
    for _ in 0..workers {
        let (job_rx, done_tx, model) = (job_rx.clone(), done_tx.clone(), model.clone());
        handles.push(std::thread::spawn(move || {
            let mut det = model.and_then(|m| rustface::create_detector(m.to_str()?).ok());
            if let Some(d) = det.as_mut() {
                // Avatars and photos on screen are small and score low; 0.8
                // still finds them without marking UI as faces.
                d.set_min_face_size(20);
                d.set_score_thresh(0.8);
                d.set_pyramid_scale_factor(0.8);
                d.set_slide_window_step(4, 4);
            }
            loop {
                let job = job_rx.lock().unwrap().recv();
                let Ok((i, frame)) = job else { break };
                let words = text.then(|| ocr(&frame, w, h).unwrap_or_default());
                let found = det.as_mut().map(|d| {
                    let (g, sw, sh) = shrink(frame.to_vec(), w, h, 1080);
                    faces(d.as_mut(), &g, sw, sh)
                });
                let _ = done_tx.send((i, words, found));
            }
        }));
    }
    drop(done_tx);

    let total = (rec.duration * RATE).ceil().max(1.0);
    let mut buf = vec![0u8; (w * h) as usize];
    let mut last_thumb: Option<Vec<u8>> = None;
    let mut times = vec![];
    let mut i = 0;
    while stdout.read_exact(&mut buf).is_ok() {
        let th = thumb(&buf, w, h);
        let new = last_thumb.as_ref().is_none_or(|p| changed(p, &th));
        if new {
            last_thumb = Some(th);
            job_tx.send((i, Arc::new(buf.clone()))).map_err(|_| anyhow!("analysis stopped"))?;
        }
        times.push(i as f64 / RATE);
        i += 1;
        progress.store(((i as f64 / total).min(0.99) * 1000.0) as u32, Ordering::Relaxed);
    }
    let decoded = child.wait().is_ok_and(|s| s.success());
    drop(job_tx);
    if !decoded || i == 0 {
        // Dropping job_tx ends the workers; their results don't matter.
        return Err(anyhow!("couldn't read the screen recording"));
    }
    let mut words: Vec<Option<Vec<Word>>> = vec![None; i];
    let mut found: Vec<Option<Vec<[f64; 4]>>> = vec![None; i];
    for (k, wd, fc) in done_rx {
        words[k] = wd;
        found[k] = fc;
    }
    // A worker that died would leave frames unread: fail rather than
    // export with things left uncovered.
    if handles.into_iter().any(|h| h.join().is_err()) {
        return Err(anyhow!("looking for sensitive data failed"));
    }
    if text {
        an.text = Some(times.iter().zip(&mut words).map(|(t, w)| Sample { t: *t, found: w.take() }).collect());
    }
    if face {
        an.faces = Some(times.iter().zip(&mut found).map(|(t, f)| Sample { t: *t, found: f.take() }).collect());
    }
    an.save(&rec.dir)?;
    progress.store(1000, Ordering::Relaxed);
    Ok(an)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(String, Sensitive)> {
        let chars: Vec<char> = line.chars().collect();
        find_sensitive(line, &["Northwind".into()]).into_iter().map(|(a, b, k)| (chars[a..b].iter().collect(), k)).collect()
    }

    #[test]
    fn finds_emails_numbers_keys_and_phrases() {
        assert_eq!(kinds("Email jane.doe@example.com"), [("jane.doe@example.com".into(), Sensitive::Email)]);
        // OCR garbled the dot: still an email.
        assert_eq!(kinds("biling@acme-corpiio"), [("biling@acme-corpiio".into(), Sensitive::Email)]);
        assert_eq!(kinds("call +1 (415) 555-0132 now"), [("+1 (415) 555-0132".into(), Sensitive::Number)]);
        assert_eq!(kinds("4242 4242 4242 4242"), [("4242 4242 4242 4242".into(), Sensitive::Number)]);
        assert_eq!(kinds("020 7946 0958"), [("020 7946 0958".into(), Sensitive::Number)]);
        assert_eq!(kinds("sk-live-9fA3kQ2mZx81LpTn4vB7wYc0"), [("sk-live-9fA3kQ2mZx81LpTn4vB7wYc0".into(), Sensitive::Key)]);
        assert_eq!(kinds("token ghp_abcdefghijklmnop1234"), [("ghp_abcdefghijklmnop1234".into(), Sensitive::Key)]);
        assert_eq!(kinds("Client: northwind traders"), [("northwind".into(), Sensitive::Phrase)]);
        // Not sensitive: dates, times, prices, short numbers, links, paths, words.
        for s in [
            "2026-10-24 13:45",
            "Business - 24 seats",
            "$1,299.00",
            "v4.1.0-5005",
            "https://github.com/IHAVENOARMS/open-studio/pull/12",
            "/home/user/projects/open-studio/src/editor/mod.rs",
            "Internationalization",
            "Order 12345",
        ] {
            assert!(kinds(s).is_empty(), "{s}: {:?}", kinds(s));
        }
    }

    fn word(text: &str, line: u32, x: f64, y: f64, w: f64) -> Word {
        Word { text: text.into(), line, rect: [x, y, w, 0.02] }
    }

    #[test]
    fn covers_follow_scrolling_text() {
        // An email scrolls up from y=0.5 to y=0.3 over three looks, then
        // goes away; a plain word never gets a cover.
        let look = |y: f64| vec![word("Email", 1, 0.1, y, 0.05), word("jane@example.com", 1, 0.5, y, 0.15), word("Plan", 2, 0.1, y + 0.1, 0.04)];
        let an = Analysis {
            text: Some(vec![
                Sample { t: 0.0, found: Some(look(0.5)) },
                Sample { t: 0.25, found: None }, // unchanged
                Sample { t: 0.5, found: Some(look(0.4)) },
                Sample { t: 0.75, found: Some(look(0.3)) },
                Sample { t: 1.0, found: Some(vec![word("Plan", 2, 0.1, 0.2, 0.04)]) },
            ]),
            faces: None,
        };
        let covers = text_covers(&an, &[], (1920.0, 1080.0));
        assert_eq!(covers.len(), 1, "{covers:?}");
        let c = &covers[0];
        // Held still while the frame didn't change, then moving with the text.
        let y = |t: f64| c.at(t).unwrap()[1] + c.at(t).unwrap()[3] / 2.0;
        assert!((y(0.1) - 0.51).abs() < 1e-6 && (y(0.25) - 0.51).abs() < 1e-6);
        assert!((y(0.375) - 0.46).abs() < 1e-6, "halfway between looks: {}", y(0.375));
        // Covers the word with a margin.
        let r = c.at(0.0).unwrap();
        assert!(r[0] < 0.5 && r[0] + r[2] > 0.65 && r[1] < 0.5 && r[1] + r[3] > 0.52);
        // Showing a little past the last look that saw it, then gone.
        assert!(c.at(0.75 + PAD * 0.9).is_some());
        assert!(c.at(0.75 + PAD * 1.1).is_none());
        assert!(c.at(-PAD * 1.1).is_none());
        // A phrase added later is found without a new pass.
        let covers = text_covers(&an, &["plan".into()], (1920.0, 1080.0));
        assert_eq!(covers.len(), 2);
    }

    #[test]
    fn faces_are_followed_and_enlarged() {
        let an = Analysis {
            text: None,
            faces: Some(vec![
                Sample { t: 0.0, found: Some(vec![[0.1, 0.1, 0.05, 0.05], [0.8, 0.1, 0.05, 0.05]]) },
                Sample { t: 0.25, found: Some(vec![[0.1, 0.05, 0.05, 0.05], [0.8, 0.05, 0.05, 0.05]]) },
            ]),
        };
        let covers = face_covers(&an);
        assert_eq!(covers.len(), 2);
        assert!(covers.iter().all(|c| c.keys.len() == 2 && c.kind == CoverKind::Face));
        let r = covers[0].at(0.0).unwrap();
        assert!(r[2] > 0.05 && r[0] < 0.1);
    }

    #[test]
    fn tesseract_tsv_becomes_words() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   4\t1\t1\t1\t1\t0\t10\t20\t300\t15\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t20\t40\t15\t91.0\tEmail\n\
                   5\t1\t1\t1\t1\t2\t100\t20\t150\t15\t13.6\tjane@x.io\n\
                   5\t1\t2\t1\t1\t1\t10\t60\t40\t15\t90.0\tPlan\n";
        let w = parse_tsv(tsv, 1000.0, 100.0);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].line, w[1].line);
        assert_ne!(w[0].line, w[2].line);
        assert_eq!(w[1].rect, [0.1, 0.2, 0.15, 0.15]);
    }

    fn test_recording(dir: &Path, w: u32, h: u32, duration: f64) -> Recording {
        Recording {
            version: 1,
            video: "screen.mp4".into(),
            width: w,
            height: h,
            fps: 30,
            duration,
            cursor_embedded: false,
            source: Default::default(),
            events: Default::default(),
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: dir.to_path_buf(),
            transcript: None,
        }
    }

    /// The real pass on a made-up page that scrolls: the email's cover is
    /// where the email is before, during and after the scroll, and the
    /// plain text gets none. Opt-in (needs ffmpeg and tesseract):
    /// `cargo test -- --ignored scrolling_page`.
    #[test]
    #[ignore]
    fn scrolling_page_is_covered() {
        let dir = std::env::temp_dir().join("open-studio-privacy-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Still for 1 s, scrolls up 300 px over 2 s, still again.
        let y = "500-300*clip((t-1)/2\\,0\\,1)";
        let vf = format!(
            "drawtext=text='Email':x=300:y={y}:fontsize=20:fontcolor=white,\
             drawtext=text='jane.doe@example.com':x=900:y={y}:fontsize=20:fontcolor=0xbbbbbb,\
             drawtext=text='Plan  Business':x=300:y={y}+80:fontsize=20:fontcolor=white"
        );
        assert!(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x1e1e22:s=1920x1080:r=30:d=4", "-vf", &vf, "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(dir.join("screen.mp4"))
            .status()
            .unwrap()
            .success());
        let rec = test_recording(&dir, 1920, 1080, 4.0);
        let progress = AtomicU32::new(0);
        let an = analyze(&rec, true, false, &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 1000);
        let text = an.text.as_ref().unwrap();
        // Unchanged frames aren't read again.
        let read = text.iter().filter(|s| s.found.is_some()).count();
        assert!(read < text.len() * 3 / 4, "read {read} of {} looks", text.len());
        let covers = text_covers(&an, &[], (1920.0, 1080.0));
        assert_eq!(covers.len(), 1, "{covers:?}");
        let c = &covers[0];
        for (t, top) in [(0.5, 500.0), (2.0, 350.0), (3.5, 200.0)] {
            let r = c.at(t).unwrap_or_else(|| panic!("no cover at {t}s"));
            let (cx, cy) = ((r[0] + r[2] / 2.0) * 1920.0, (r[1] + r[3] / 2.0) * 1080.0);
            assert!((cy - (top + 10.0)).abs() < 25.0, "at {t}s the cover is at y={cy:.0}, the email at {}", top + 10.0);
            assert!(cx > 900.0 && cx < 1200.0, "x={cx:.0}");
            assert!(r[0] * 1920.0 < 900.0 && (r[0] + r[2]) * 1920.0 > 1090.0, "covers the whole email: {r:?}");
        }
        // Cached: a second call doesn't look again.
        let again = analyze(&rec, true, false, &AtomicU32::new(0)).unwrap();
        assert_eq!(again.text.unwrap().len(), text.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Faces in a photo moving across the screen are found and followed.
    /// Opt-in (downloads a public-domain portrait and the face model):
    /// `cargo test -- --ignored moving_photo`.
    #[test]
    #[ignore]
    fn moving_photo_face_is_blurred() {
        let dir = std::env::temp_dir().join("open-studio-privacy-face-test");
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("portrait.jpg");
        if !photo.exists() {
            assert!(Command::new("curl")
                .args(["-fsSL", "-A", "open-studio-test", "-o"])
                .arg(&photo)
                .arg("https://upload.wikimedia.org/wikipedia/commons/d/d3/Albert_Einstein_Head.jpg")
                .status()
                .unwrap()
                .success());
        }
        // A 160 px tall photo sliding right over 3 s, like an avatar in a call.
        assert!(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x202024:s=1280x720:r=30:d=3", "-i"])
            .arg(&photo)
            .args(["-filter_complex", "[1]scale=-2:160[p];[0][p]overlay=x='100+200*t':y=200", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(dir.join("screen.mp4"))
            .status()
            .unwrap()
            .success());
        let _ = std::fs::remove_file(dir.join(FILE));
        let rec = test_recording(&dir, 1280, 720, 3.0);
        let an = analyze(&rec, false, true, &AtomicU32::new(0)).unwrap();
        let covers = face_covers(&an);
        assert!(!covers.is_empty(), "no face found");
        let c = covers.iter().max_by_key(|c| c.keys.len()).unwrap();
        let x = |t: f64| c.at(t).map(|r| (r[0] + r[2] / 2.0) * 1280.0);
        let (a, b) = (x(0.5).expect("face at 0.5s"), x(2.5).expect("face at 2.5s"));
        assert!((b - a - 400.0).abs() < 60.0, "the blur moved {:.0}px with a face that moved 400px", b - a);
    }
}
