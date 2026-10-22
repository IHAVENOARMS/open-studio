//! Captions and Transcript panels: generating a transcript with Whisper,
//! caption styling, and editing the video by editing its transcript.

use super::sidebar::{heading, help, sub_card, toggle_help};
use super::widgets;
use crate::bidi;
use crate::edit::{CaptionAnimation, CaptionDisplay, CaptionFont, CaptionPosition, EditConfig};
use crate::icons::Icon;
use crate::project::Recording;
use crate::theme;
use crate::transcript::{self, PAUSE_KEEP, Transcript};
use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};
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
    /// Selected words, inclusive, in transcript order.
    selection: Option<(usize, usize)>,
    anchor: Option<usize>,
    /// Word being retyped.
    editing: Option<(usize, String)>,
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
            st.selection = None;
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
fn word_span(t: &Transcript, a: usize, b: usize) -> (f64, f64) {
    let w = &t.words;
    let lo = if a > 0 { w[a].start - ((w[a].start - w[a - 1].end) * 0.5).min(0.08) } else { (w[a].start - 0.08).max(0.0) };
    let hi = if b + 1 < w.len() { w[b].end + ((w[b + 1].start - w[b].end) * 0.5).min(0.08) } else { w[b].end + 0.08 };
    (lo, hi)
}

/// The Transcript panel. Returns a time to seek to.
pub fn transcript(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording, time: f64) -> Option<f64> {
    poll_job(ui, st);
    heading(ui, "Edit by transcript");
    let Some(t) = &rec.transcript else {
        help(ui, "Generate a transcript in the Captions tab first.");
        return None;
    };
    let mut seek = None;

    // Bulk actions.
    let fillers: Vec<usize> = t.fillers().into_iter().filter(|i| !edit.is_cut(t.words[*i].start, t.words[*i].end)).collect();
    let pauses: Vec<(f64, f64)> = t
        .pauses()
        .into_iter()
        .map(|(_, a, b)| (a + PAUSE_KEEP, b - PAUSE_KEEP))
        .filter(|(a, b)| b > a && edit.cut_at((a + b) * 0.5).is_none())
        .collect();
    let r = widgets::wide_button_icon(ui, Some(Icon::RailAudio), &format!("Remove {} filler words", fillers.len()))
        .on_hover_text("Cut every detected filled pause (\"um\", \"uh\"…) out of the video");
    if r.clicked() {
        for i in &fillers {
            let (a, b) = word_span(t, *i, *i);
            edit.add_cut(a, b);
        }
    }
    let r = widgets::wide_button_icon(ui, Some(Icon::Aspect), &format!("Tighten {} pauses", pauses.len()))
        .on_hover_text("Shorten every long pause, keeping a natural breath on each side");
    if r.clicked() {
        for (a, b) in &pauses {
            edit.add_cut(*a, *b);
        }
    }
    ui.add_space(6.0);

    // Selection actions.
    if let Some((a, b)) = st.selection.filter(|(_, b)| *b < t.words.len()) {
        let (lo, hi) = word_span(t, a, b);
        let all_cut = (a..=b).all(|i| edit.is_cut(t.words[i].start, t.words[i].end) || edit.captions.hidden_words.contains(&i));
        sub_card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if all_cut {
                    if widgets::chip(ui, "Restore", false).clicked() {
                        edit.restore(lo, hi);
                        edit.captions.hidden_words.retain(|i| *i < a || *i > b);
                        st.selection = None;
                    }
                } else {
                    if widgets::chip(ui, "Cut video", false).on_hover_text("Cut this part out of the video  (Backspace)").clicked() {
                        edit.add_cut(lo, hi);
                        st.selection = None;
                    }
                    if widgets::chip(ui, "Cut transcript", false).on_hover_text("Remove these words from the captions only  (Ctrl+Backspace)").clicked() {
                        edit.captions.hidden_words.extend(a..=b);
                        edit.captions.hidden_words.sort_unstable();
                        edit.captions.hidden_words.dedup();
                        st.selection = None;
                    }
                }
                if widgets::chip(ui, "Play", false).clicked() {
                    seek = Some(t.words[a].start);
                    st.play_request = true;
                }
                if a == b && widgets::chip(ui, "Edit text", false).clicked() {
                    st.editing = Some((a, edit.captions.word_text(a, &t.words[a].text).to_string()));
                }
            });
        });
        // Keyboard: Backspace cuts the video, Ctrl+Backspace the transcript only.
        if !all_cut && st.editing.is_none() && ui.memory(|m| m.focused().is_none()) {
            let ctrl = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Backspace));
            let plain = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) || i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
            });
            if ctrl {
                edit.captions.hidden_words.extend(a..=b);
                edit.captions.hidden_words.sort_unstable();
                edit.captions.hidden_words.dedup();
                st.selection = None;
            } else if plain {
                edit.add_cut(lo, hi);
                st.selection = None;
            }
        }
        ui.add_space(4.0);
    }

    // Transcript body: words flow as text; fillers underlined, long pauses
    // as pills, cut words struck through.
    let current = t.words.iter().rposition(|w| w.start <= time && time < w.end + 0.15);
    let pause_before: std::collections::HashMap<usize, (f64, f64)> = t.pauses().into_iter().map(|(i, a, b)| (i, (a, b))).collect();
    let font = egui::FontId::proportional(14.0);
    let shift = ui.input(|i| i.modifiers.shift);
    let mut clicked_word = None;
    ui.add_space(4.0);
    // Arabic transcripts flow right to left: lines are broken in reading
    // order, then each line's words are set right to left with English runs
    // inside them left to right.
    let rtl = bidi::paragraph_rtl(t.words.iter().map(|w| w.text.as_str()));
    let visible: Vec<usize> = (0..t.words.len())
        .filter(|&i| edit.captions.show_deleted || !(edit.is_cut(t.words[i].start, t.words[i].end) || edit.captions.hidden_words.contains(&i)))
        .collect();
    let st_editing = st.editing.as_ref().map(|(i, _)| *i);
    let mut draw = |ui: &mut Ui, i: usize| {
        let w = &t.words[i];
        let cut = edit.is_cut(w.start, w.end);
        let hidden = edit.captions.hidden_words.contains(&i);
        if (cut || hidden) && !edit.captions.show_deleted {
            return;
        }
        if let Some(&(a, b)) = pause_before.get(&i) {
            let tightened = edit.cut_at((a + b) * 0.5).is_some();
            let label = format!("··· {:.1}s", if tightened { 2.0 * PAUSE_KEEP } else { b - a });
            let g = ui.painter().layout_no_wrap(label, egui::FontId::proportional(11.5), Color32::from_rgb(0x9e, 0x91, 0xf3));
            let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 20.0), Sense::click());
            ui.painter().rect_filled(r.shrink2(vec2(2.0, 0.0)), CornerRadius::same(6), Color32::from_rgb(0x22, 0x20, 0x33));
            ui.painter().galley(r.center() - g.size() / 2.0, g, Color32::WHITE);
            let resp = resp.on_hover_text(if tightened { "Restore this pause" } else { "Tighten this pause" });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if tightened {
                    edit.restore(a, b);
                } else {
                    edit.add_cut(a + PAUSE_KEEP, b - PAUSE_KEEP);
                }
            }
            ui.add_space(4.0);
        }
        if let Some((ei, text)) = &mut st.editing {
            if *ei == i {
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(90.0).font(font.clone()));
                r.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) || r.lost_focus() {
                    let new = text.trim().to_string();
                    if new.is_empty() || new == w.text {
                        edit.captions.word_edits.remove(&i);
                    } else {
                        edit.captions.word_edits.insert(i, new);
                    }
                    st.editing = None;
                } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    st.editing = None;
                }
                ui.add_space(4.0);
                return;
            }
        }
        // The gap goes on the side the next word follows.
        let word = edit.captions.word_text(i, &w.text);
        let text = if rtl { format!(" {word}") } else { format!("{word} ") };
        let filler = transcript::is_filler(&w.text);
        let color = if cut || hidden {
            Color32::from_rgb(0x5a, 0x5b, 0x62)
        } else if filler {
            Color32::from_rgb(0xb4, 0xa9, 0xf7)
        } else {
            theme::TEXT
        };
        let g = ui.painter().layout_no_wrap(text, font.clone(), color);
        let (r, resp) = ui.allocate_exact_size(g.size(), Sense::click());
        let selected = st.selection.is_some_and(|(a, b)| i >= a && i <= b);
        let p = ui.painter();
        if selected {
            p.rect_filled(r.expand2(vec2(1.0, 1.0)), CornerRadius::same(3), Color32::from_rgb(0x3a, 0x30, 0x7a));
        } else if current == Some(i) {
            p.rect_filled(r.expand2(vec2(1.0, 1.0)), CornerRadius::same(3), Color32::from_rgb(0x2a, 0x26, 0x48));
        } else if resp.hovered() {
            p.rect_filled(r.expand2(vec2(1.0, 1.0)), CornerRadius::same(3), Color32::from_white_alpha(10));
        }
        let gap = ui.fonts_mut(|f| f.glyph_width(&font, ' '));
        let (x0, x1) = if rtl { (r.left() + gap, r.right()) } else { (r.left(), r.right() - gap) };
        p.galley(r.min, g, color);
        if filler && !cut {
            let y = r.bottom() - 1.0;
            p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, Color32::from_rgb(0x8b, 0x7c, 0xf0)));
        }
        if cut || hidden {
            let y = r.center().y;
            p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, Color32::from_rgb(0x6a, 0x6b, 0x72)));
        }
        if resp.double_clicked() {
            st.editing = Some((i, edit.captions.word_text(i, &w.text).to_string()));
        } else if resp.clicked() {
            clicked_word = Some(i);
        }
        resp.on_hover_cursor(egui::CursorIcon::Text);
    };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
        if !rtl {
            ui.horizontal_wrapped(|ui| {
                for &i in &visible {
                    draw(ui, i);
                }
            });
            return;
        }
        let pill_font = egui::FontId::proportional(11.5);
        let item_width = |ui: &Ui, i: usize| {
            let word = ui.painter().layout_no_wrap(format!(" {}", t.words[i].text), font.clone(), Color32::WHITE).size().x;
            let pill = pause_before.get(&i).map_or(0.0, |(a, b)| ui.painter().layout_no_wrap(format!("··· {:.1}s", b - a), pill_font.clone(), Color32::WHITE).size().x + 18.0);
            // A word being corrected is a text box.
            if st_editing == Some(i) { word.max(94.0) + pill } else { word + pill }
        };
        let width = ui.available_width();
        let mut rows: Vec<Vec<usize>> = vec![];
        let mut used = f32::INFINITY;
        for &i in &visible {
            let w = item_width(ui, i);
            if used + w > width {
                rows.push(vec![]);
                used = 0.0;
            }
            rows.last_mut().unwrap().push(i);
            used += w;
        }
        for row in rows {
            let texts: Vec<&str> = row.iter().map(|&i| t.words[i].text.as_str()).collect();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                for k in bidi::visual_order(&texts, true).into_iter().rev() {
                    draw(ui, row[k]);
                }
            });
        }
    });
    if let Some(i) = clicked_word {
        match (shift, st.anchor) {
            (true, Some(a)) => st.selection = Some((a.min(i), a.max(i))),
            _ => {
                st.anchor = Some(i);
                st.selection = Some((i, i));
                seek = Some(t.words[i].start);
            }
        }
    }
    ui.add_space(12.0);
    help(ui, "Click a word to jump to it, Shift+click to select a range. Backspace cuts the words out of the video, Ctrl+Backspace cuts them from the transcript only. Double-click a word to correct it.");
    widgets::divider(ui);
    toggle_help(ui, "Show deleted text", "Keep cut words visible as strikethrough so they can be restored.", &mut edit.captions.show_deleted);
    seek
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
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(380.0, 900.0))),
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
        // Select "this" via the selection state and cut it with Backspace.
        st.selection = Some((2, 2));
        frame(&ctx, vec![egui::Event::Key { key: egui::Key::Backspace, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }], &mut |ui| {
            transcript(ui, &mut st, &mut edit, &rec, 0.0);
        });
        assert!(edit.is_cut(0.8, 1.1), "cuts: {:?}", edit.cuts);
        // Restore it.
        edit.restore(0.75, 1.15);
        assert!(!edit.is_cut(0.8, 1.1));
    }
}
