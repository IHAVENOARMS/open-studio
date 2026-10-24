//! The Privacy section (Background & Screen tab): Hide sensitive data with
//! custom phrases, and Hide faces. Turning one on looks through the
//! recording in the background (see `crate::privacy`); the preview shows
//! the progress.

use super::widgets;
use crate::edit::EditConfig;
use crate::project::Recording;
use crate::theme;
use egui::{Color32, CornerRadius, Sense, Ui, vec2};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

#[derive(Default)]
pub struct State {
    job: Option<Job>,
    /// What the cached analysis has: (text, faces), once checked.
    have: Option<(bool, bool)>,
    /// Why the last pass failed, and for which settings.
    error: Option<(String, (bool, bool))>,
    /// "Add phrase" field.
    phrase: String,
}

struct Job {
    progress: Arc<AtomicU32>,
    rx: Receiver<Result<(), String>>,
    text: bool,
}

/// Starts a pass when a toggle is on and its part hasn't been looked for,
/// and collects finished passes. True when covers changed (rebuild tracks).
pub fn tick(state: &mut State, edit: &EditConfig, rec: &Recording, ctx: &egui::Context) -> bool {
    let p = &edit.privacy;
    if let Some(job) = &state.job {
        match job.rx.try_recv() {
            Ok(res) => {
                state.job = None;
                state.have = None;
                if let Err(e) = res {
                    state.error = Some((e, (p.hide_sensitive, p.hide_faces)));
                }
                return true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.job = None;
                state.error = Some(("looking for sensitive data stopped".into(), (p.hide_sensitive, p.hide_faces)));
                return true;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return false;
            }
        }
    }
    if !p.hide_sensitive && !p.hide_faces {
        return false;
    }
    let have = *state.have.get_or_insert_with(|| {
        let an = crate::privacy::Analysis::load(&rec.dir);
        (an.text.is_some(), an.faces.is_some())
    });
    let text = p.hide_sensitive && !have.0;
    let faces = p.hide_faces && !have.1;
    if !(text || faces) || state.error.as_ref().is_some_and(|e| e.1 == (p.hide_sensitive, p.hide_faces)) {
        return false;
    }
    let progress = Arc::new(AtomicU32::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let (r, pr, ctx2) = (rec.clone(), progress.clone(), ctx.clone());
    std::thread::spawn(move || {
        let res = crate::privacy::analyze(&r, text, faces, &pr).map(|_| ()).map_err(|e| format!("{e:#}"));
        let _ = tx.send(res);
        ctx2.request_repaint();
    });
    state.job = Some(Job { progress, rx, text });
    state.error = None;
    false
}

/// Text and progress (0..1) for the preview while a pass runs.
pub fn status(state: &State) -> Option<(String, f32)> {
    let job = state.job.as_ref()?;
    let p = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
    let what = if job.text { "Looking for sensitive data…" } else { "Looking for faces…" };
    Some((if p == 0.0 { what.to_string() } else { format!("{what}  {:.0}%", p * 100.0) }, p))
}

/// The Privacy section.
pub fn section(ui: &mut Ui, state: &mut State, edit: &mut EditConfig) {
    widgets::divider(ui);
    theme::section(ui, "Privacy");
    let p = &mut edit.privacy;
    widgets::toggle_row(ui, "Hide sensitive data", &mut p.hide_sensitive);
    help(ui, "Covers emails, phone and card numbers, and keys that appear on screen. Covers follow the text while you scroll and type.");
    if p.hide_sensitive {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Also hide these phrases").size(13.0).color(theme::TEXT));
        ui.add_space(2.0);
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (i, phrase) in p.phrases.iter().enumerate() {
                if widgets::chip(ui, &format!("{phrase}  ×"), false).on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            }
        });
        if let Some(i) = remove {
            p.phrases.remove(i);
        }
        ui.add_space(4.0);
        let field = egui::TextEdit::singleline(&mut state.phrase)
            .hint_text("Name, client, project…  (Enter to add)")
            .desired_width(ui.available_width());
        let r = ui.add(field);
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let phrase = state.phrase.trim().to_string();
            if !phrase.is_empty() && !p.phrases.iter().any(|q| q.eq_ignore_ascii_case(&phrase)) {
                p.phrases.push(phrase);
            }
            state.phrase.clear();
            r.request_focus();
        }
    }
    ui.add_space(8.0);
    widgets::toggle_row(ui, "Hide faces", &mut p.hide_faces);
    help(ui, "Blurs faces on screen, like avatars, photos, and people in video calls. Each blur stays on its face while the page scrolls.");
    if let Some((e, _)) = &state.error {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Couldn't look through the recording: {e}")).size(12.5).color(Color32::from_rgb(0xff, 0x8a, 0x80)));
        if widgets::wide_button(ui, "Try again").clicked() {
            state.error = None;
        }
    }
    if p.hide_sensitive || p.hide_faces {
        ui.add_space(8.0);
        review_note(ui);
    }
}

/// Screen Studio's "Review Before Sharing" reminder.
pub fn review_note(ui: &mut Ui) {
    let w = ui.available_width();
    let galley = ui.painter().layout(
        "Review before sharing: some private details may still be visible, so watch your recording in full before you share it.".into(),
        egui::FontId::proportional(12.5),
        theme::TEXT,
        w - 20.0,
    );
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 16.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(8), Color32::from_rgba_unmultiplied(0xf5, 0xc4, 0x51, 22));
    ui.painter().rect_stroke(r, CornerRadius::same(8), egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xf5, 0xc4, 0x51, 70)), egui::StrokeKind::Inside);
    ui.painter().galley(r.min + vec2(10.0, 8.0), galley, theme::TEXT);
}

fn help(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.0).color(theme::TEXT_DIM));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass starts when a toggle is turned on, a failed pass reports why
    /// and isn't retried on its own, and nothing runs while both are off.
    #[test]
    fn pass_starts_fails_once_and_waits() {
        let dir = std::env::temp_dir().join("open-studio-privacy-ui-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No screen video: the pass can't read it.
        let rec = Recording {
            version: 1,
            video: "missing.mp4".into(),
            width: 1280,
            height: 720,
            fps: 30,
            duration: 2.0,
            cursor_embedded: false,
            source: Default::default(),
            events: Default::default(),
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: dir.clone(),
            transcript: None,
        };
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut edit = EditConfig::default();
        assert!(!tick(&mut state, &edit, &rec, &ctx));
        assert!(state.job.is_none(), "nothing to do while both are off");
        edit.privacy.hide_faces = true;
        tick(&mut state, &edit, &rec, &ctx);
        assert!(state.job.is_some());
        assert!(status(&state).unwrap().0.starts_with("Looking for faces"));
        let start = std::time::Instant::now();
        while !tick(&mut state, &edit, &rec, &ctx) {
            assert!(start.elapsed().as_secs() < 30, "pass never finished");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(state.job.is_none());
        assert!(state.error.as_ref().is_some_and(|e| e.0.contains("couldn't read")), "{:?}", state.error);
        // Not retried in a loop.
        tick(&mut state, &edit, &rec, &ctx);
        assert!(state.job.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
