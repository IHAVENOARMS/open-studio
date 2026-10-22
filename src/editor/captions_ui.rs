//! Captions and Transcript panels: generating a transcript with Whisper,
//! caption styling, and editing the video by editing its transcript.

use super::sidebar::{heading, help, toggle_help};
use super::widgets;
use crate::edit::{CaptionAnimation, CaptionDisplay, CaptionFont, CaptionPosition, EditConfig};
use crate::icons::Icon;
use crate::project::Recording;
use crate::theme;
use crate::transcript::{self, Transcript};
use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

struct Job {
    progress: Arc<transcript::Progress>,
    rx: Receiver<Result<Transcript, String>>,
}

#[derive(Default)]
pub struct State {
    job: Option<Job>,
    error: Option<String>,
    /// A transcript that just finished; the editor picks it up.
    pub finished: Option<Transcript>,
    /// Show the generate form even though a transcript exists.
    regenerate: bool,
    /// The transcript editor's caret and selection.
    pub(super) doc: super::transcript_ui::Doc,
    /// A message for the editor's bottom toast ("Removed 6.0s …").
    pub toast: Option<String>,
    /// Set by "Play": the editor starts playback from the seek point.
    pub play_request: bool,
    saving_srt: Option<Receiver<Vec<PathBuf>>>,
    srt_message: Option<String>,
}

/// What the Captions panel asks the sidebar to do.
pub enum CaptionsAction {
    None,
    OpenTranscript,
}

const COLORS: [[f32; 3]; 5] = [
    [1.0, 1.0, 1.0],
    [1.0, 0.84, 0.04],
    [0.62, 0.57, 0.95],
    [0.19, 0.82, 0.35],
    [0.39, 0.82, 1.0],
];

fn rgb(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

fn poll_job(ui: &Ui, st: &mut State) {
    let Some(job) = &st.job else { return };
    match job.rx.try_recv() {
        Ok(Ok(t)) => {
            st.finished = Some(t);
            st.job = None;
            st.regenerate = false;
            st.doc = Default::default();
        }
        Ok(Err(e)) => {
            st.error = Some(e);
            st.job = None;
        }
        Err(TryRecvError::Disconnected) => st.job = None,
        Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(150)),
    }
}

fn start_job(ui: &Ui, st: &mut State, edit: &EditConfig, rec: &Recording) {
    let progress = Arc::new(transcript::Progress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let (rec, c, p, ctx) = (rec.clone(), edit.captions.clone(), progress.clone(), ui.ctx().clone());
    std::thread::spawn(move || {
        let r = transcript::generate(&rec, &c.model, &c.language, &c.prompt, &p)
            .and_then(|t| t.save(&rec.dir).map(|_| t))
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    st.job = Some(Job { progress, rx });
    st.error = None;
}

impl State {
    pub fn transcribing(&self) -> bool {
        self.job.is_some()
    }
}

pub fn captions(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording) -> CaptionsAction {
    poll_job(ui, st);
    heading(ui, "Captions");
    let mut action = CaptionsAction::None;
    if rec.transcript.is_none() || st.regenerate || st.job.is_some() {
        generate_form(ui, st, edit, rec);
        return action;
    }
    let c = &mut edit.captions;
    toggle_help(ui, "Show captions", "Burn the transcript into the video as captions.", &mut c.show);
    if c.show {
        theme::section(ui, "Captions size");
        widgets::slider(ui, &mut c.size, 0.5..=2.0, Some(1.0));
        theme::section(ui, "Font style");
        let fonts: Vec<_> = CaptionFont::ALL.iter().map(|f| (*f, f.label())).collect();
        widgets::segmented(ui, &mut c.font, &fonts);
        theme::section(ui, "Color");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for col in COLORS {
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                let selected = c.color == col;
                ui.painter().circle_filled(r.center(), 10.0, rgb(col));
                if selected {
                    ui.painter().circle_stroke(r.center(), 12.5, Stroke::new(1.5, theme::KNOB));
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    c.color = col;
                }
            }
        });
        theme::section(ui, "Position");
        widgets::segmented(ui, &mut c.position, &[(CaptionPosition::Bottom, "Bottom"), (CaptionPosition::Top, "Top")]);
        theme::section(ui, "Display");
        widgets::segmented(ui, &mut c.display, &[(CaptionDisplay::LineByLine, "Line-by-line"), (CaptionDisplay::WordByWord, "Word-by-word")]);
        theme::section(ui, "Animation");
        widgets::segmented(
            ui,
            &mut c.animation,
            &[(CaptionAnimation::Appear, "Appear"), (CaptionAnimation::Fade, "Fade in"), (CaptionAnimation::Slide, "Slide in")],
        );
    }
    widgets::divider(ui);
    if widgets::wide_button_icon(ui, Some(Icon::RailTranscript), "Edit transcript").clicked() {
        action = CaptionsAction::OpenTranscript;
    }
    // Export .srt.
    if let Some(rx) = &st.saving_srt {
        match rx.try_recv() {
            Ok(files) => {
                if let (Some(path), Some(t)) = (files.into_iter().next(), &rec.transcript) {
                    let text = transcript::srt(t, edit, rec.duration);
                    st.srt_message = Some(match std::fs::write(&path, text) {
                        Ok(()) => format!("Saved {}", path.display()),
                        Err(e) => format!("Could not save: {e}"),
                    });
                }
                st.saving_srt = None;
            }
            Err(TryRecvError::Disconnected) => st.saving_srt = None,
            Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    if widgets::wide_button_icon(ui, Some(Icon::File), "Export transcript (.srt)").clicked() && st.saving_srt.is_none() {
        let dir = crate::settings::Settings::load().export_dir;
        let name = format!("{}.srt", rec.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("captions".into()));
        st.saving_srt = Some(crate::dialogs::save_file(ui.ctx().clone(), "Export transcript", &dir, &name));
    }
    if let Some(m) = &st.srt_message {
        help(ui, m);
    }
    if widgets::wide_button_icon(ui, Some(Icon::Restart), "Generate transcript again").clicked() {
        st.regenerate = true;
    }
    action
}

fn generate_form(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording) {
    let c = &mut edit.captions;
    ui.label(RichText::new("Generate transcript").size(12.5).color(theme::TEXT_DIM));
    theme::section(ui, "Transcript method");
    let mut method = 0;
    widgets::segmented(ui, &mut method, &[(0, "Whisper")]);
    help(ui, "Transcript will be generated locally using the Whisper AI model. No data will be sent to the server.");

    theme::section(ui, "Language");
    let current = transcript::LANGUAGES.iter().find(|(code, _)| *code == c.language).map_or("Auto", |(_, l)| l);
    egui::ComboBox::from_id_salt("caption-language").selected_text(current).width(180.0).show_ui(ui, |ui| {
        for (code, label) in transcript::LANGUAGES {
            ui.selectable_value(&mut c.language, code.to_string(), label);
        }
    });

    theme::section(ui, "AI model to use");
    help(ui, "Better model will produce more accurate results, but will take more time and require larger file to be downloaded on first use.");
    let opts: Vec<(&str, &str)> = transcript::MODELS.iter().map(|(id, label, _)| (*id, *label)).collect();
    let mut model = transcript::MODELS.iter().map(|m| m.0).find(|m| *m == c.model).unwrap_or("base");
    if widgets::segmented(ui, &mut model, &opts) {
        c.model = model.to_string();
    }
    if let Some((_, _, h)) = transcript::MODELS.iter().find(|m| m.0 == model) {
        help(ui, h);
    }
    if c.language == "ar" && model != "large-v3-turbo" {
        help(ui, "For Arabic, especially dialects like Egyptian, use Large: smaller models often write dialect as formal Arabic or miss words.");
    }

    theme::section(ui, "Prompt");
    help(ui, "Prompt will help generating accurate transcript for your recording. It is especially useful if you talk using custom names, such as your product name.");
    ui.add(
        egui::TextEdit::multiline(&mut c.prompt)
            .hint_text("e.g. I'm talking about Open Studio, custom captions and AI transcript generation")
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(10.0);

    if let Some(job) = &st.job {
        let p = job.progress.permille.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(3), theme::TRACK);
        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, vec2(w * p.max(0.02), 6.0)), CornerRadius::same(3), theme::ACCENT);
        help(ui, &if p == 0.0 { "Loading the model…".to_string() } else { format!("Transcribing…  {:.0}%", p * 100.0) });
        if widgets::wide_button(ui, "Cancel").clicked() {
            job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return;
    }
    if !transcript::whisper_available() {
        help(ui, "Whisper is not installed. Install the openai-whisper package (for example `pipx install openai-whisper`) and reopen the project.");
        return;
    }
    let w = ui.available_width();
    let clicked = ui
        .add_sized(
            [w, 36.0],
            egui::Button::new(RichText::new("Generate transcript").font(theme::semibold(13.5)).color(Color32::WHITE))
                .fill(theme::ACCENT)
                .corner_radius(CornerRadius::same(8)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked();
    if clicked {
        start_job(ui, st, edit, rec);
    }
    if st.regenerate && rec.transcript.is_some() && widgets::wide_button(ui, "Keep the current transcript").clicked() {
        st.regenerate = false;
    }
    if let Some(e) = &st.error {
        ui.label(RichText::new(e).size(12.0).color(Color32::from_rgb(0xff, 0x6b, 0x6b)));
    }
}

/// Range to cut for words `a..=b`: from their start to their end, plus a
/// little of the silence around them so no fragment of a word is left.
pub(super) fn word_span(t: &Transcript, a: usize, b: usize) -> (f64, f64) {
    let w = &t.words;
    let lo = if a > 0 { w[a].start - ((w[a].start - w[a - 1].end) * 0.5).min(0.08) } else { (w[a].start - 0.08).max(0.0) };
    let hi = if b + 1 < w.len() { w[b].end + ((w[b + 1].start - w[b].end) * 0.5).min(0.08) } else { w[b].end + 0.08 };
    (lo, hi)
}

/// The Transcript panel.
pub fn transcript(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording, time: f64) -> super::transcript_ui::Action {
    poll_job(ui, st);
    super::transcript_ui::show(ui, st, edit, rec, time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Word;

    fn rec() -> Recording {
        let words = ["Hello,", "um,", "this", "is", "a", "test."]
            .iter()
            .enumerate()
            .map(|(i, w)| Word { start: i as f64 * 0.4 + if i >= 4 { 1.5 } else { 0.0 }, end: i as f64 * 0.4 + 0.3 + if i >= 4 { 1.5 } else { 0.0 }, text: w.to_string() })
            .collect();
        let mut r = Recording {
            version: 1,
            video: "screen.mp4".into(),
            width: 1920,
            height: 1080,
            fps: 60,
            duration: 5.0,
            cursor_embedded: false,
            source: Default::default(),
            events: Default::default(),
            custom_cursors: vec![],
            audio: vec![crate::project::AudioTrack { file: "microphone.wav".into(), kind: "microphone".into(), offset: 0.0 }],
            camera: None,
            dir: Default::default(),
            transcript: None,
        };
        r.transcript = Some(Transcript { words, ..Default::default() });
        r
    }

    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, f: &mut dyn FnMut(&mut Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(380.0, 900.0))),
            events,
            ..Default::default()
        };
        // No renderer here to upload the font atlas to.
        ctx.run_ui(input, |ui| f(ui)).textures_delta.clear();
    }

    #[test]
    fn transcript_panel_cuts_and_restores() {
        let ctx = egui::Context::default();
        let rec = rec();
        let mut edit = EditConfig::default();
        let mut st = State::default();
        // Captions panel renders with a transcript.
        frame(&ctx, vec![], &mut |ui| {
            captions(ui, &mut st, &mut edit, &rec);
        });
        frame(&ctx, vec![], &mut |ui| {
            transcript(ui, &mut st, &mut edit, &rec, 0.0);
        });
        // What "Remove filler words" does.
        for i in rec.transcript.as_ref().unwrap().fillers() {
            let (a, b) = word_span(rec.transcript.as_ref().unwrap(), i, i);
            edit.add_cut(a, b);
        }
        assert_eq!(edit.cuts.len(), 1);
        assert!(edit.is_cut(0.4, 0.7));
        // Select "this" and cut it with Backspace.
        st.doc.select_for_test(2, 2);
        ctx.memory_mut(|m| m.request_focus(super::super::transcript_ui::doc_id()));
        let backspace = || egui::Event::Key { key: egui::Key::Backspace, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        for events in [vec![], vec![backspace()]] {
            frame(&ctx, events, &mut |ui| {
                transcript(ui, &mut st, &mut edit, &rec, 0.0);
            });
        }
        assert!(edit.is_cut(0.8, 1.1), "cuts: {:?}", edit.cuts);
        assert!(st.toast.as_deref().is_some_and(|m| m.starts_with("Removed")));
        // Typing at a caret corrects the word.
        st.doc.caret_for_test(5, 4);
        frame(&ctx, vec![egui::Event::Text("ed".into())], &mut |ui| {
            transcript(ui, &mut st, &mut edit, &rec, 0.0);
        });
        assert_eq!(edit.captions.word_text(5, "test."), "tested.");
        // Restore the cut.
        edit.restore(0.75, 1.15);
        assert!(!edit.is_cut(0.8, 1.1));
    }
}
