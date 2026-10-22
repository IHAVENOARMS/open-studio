//! The Transcript panel ("Edit by transcript"), after Screen Studio's: the
//! transcript reads as paragraphs of text. Click to place a caret and type to
//! correct words; drag to select words, then cut them from the video or from
//! the captions with the menu beside the selection, or with Backspace /
//! Ctrl+Backspace. Each paragraph runs in its own direction, so Arabic reads
//! right to left with English runs inside it left to right.

use super::captions_ui::{State, word_span};
use super::sidebar::{help, toggle_help};
use super::widgets;
use crate::bidi;
use crate::edit::EditConfig;
use crate::icons::{self, Icon};
use crate::project::Recording;
use crate::theme;
use crate::transcript::{self, PAUSE_KEEP, Transcript};
use egui::{Color32, CornerRadius, Event, FontId, Galley, Key, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::sync::Arc;

/// Focus id of the transcript text, so the editor lets Ctrl+Z through to undo.
pub fn doc_id() -> egui::Id {
    egui::Id::NULL.with("transcript-doc")
}

#[derive(Default)]
pub struct Doc {
    /// Caret: word index and character offset into the word's shown text.
    caret: Option<(usize, usize)>,
    /// Selected words, inclusive, in transcript order.
    sel: Option<(usize, usize)>,
    /// Word a drag or Shift+click range starts from.
    anchor: Option<usize>,
    dragging: bool,
    /// Where the selection menu was last frame, so clicks on it don't
    /// count as clicks elsewhere.
    menu: Option<Rect>,
}

#[cfg(test)]
impl Doc {
    pub fn select_for_test(&mut self, a: usize, b: usize) {
        self.sel = Some((a, b));
    }
    pub fn caret_for_test(&mut self, w: usize, off: usize) {
        self.caret = Some((w, off));
        self.sel = None;
    }
}

#[derive(Default)]
pub struct Action {
    pub seek: Option<f64>,
    /// "Close Transcript editor": back to the Captions tab.
    pub close: bool,
}

const FONT: f32 = 14.0;
const ROW_H: f32 = 23.0;
const PARA_GAP: f32 = 12.0;
const DIM: Color32 = Color32::from_rgb(0x62, 0x63, 0x6a);
const FILLER: Color32 = Color32::from_rgb(0x7b, 0x6c, 0xff);
const FILLER_CUT: Color32 = Color32::from_rgb(0x4a, 0x42, 0x94);
const SOFT_UNDERLINE: Color32 = Color32::from_rgb(0x7c, 0x7d, 0x85);
const SEL_BG: Color32 = Color32::from_rgb(0x3b, 0x2c, 0x9e);
const SEL_TEXT: Color32 = Color32::from_rgb(0xf3, 0xbd, 0xea);
const PILL_BG: Color32 = Color32::from_rgb(0x1f, 0x20, 0x26);
const PILL_TEXT: Color32 = Color32::from_rgb(0x70, 0x71, 0x79);
const PILL_TIGHT: Color32 = Color32::from_rgb(0x8b, 0x7c, 0xf0);

#[derive(Clone, Copy)]
enum Item {
    /// A long pause before the paragraph's first word: (start, end).
    Pause(f64, f64),
    Word(usize),
}

struct Placed {
    item: Item,
    rect: Rect,
    galley: Arc<Galley>,
    row: usize,
}

struct Row {
    top: f32,
    rtl: bool,
}

/// The Transcript panel.
pub fn show(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording, time: f64) -> Action {
    let mut act = Action::default();
    if icons::button(ui, Icon::Back, "Close Transcript editor", FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 34.0)).clicked() {
        act.close = true;
    }
    let Some(t) = &rec.transcript else {
        help(ui, "Generate a transcript in the Captions tab first.");
        return act;
    };
    ui.add_space(8.0);
    ui.label(egui::RichText::new("Edit by transcript").size(12.5).color(theme::TEXT_DIM));

    // Bulk actions, shown while there's something to do.
    let fillers: Vec<usize> = t.fillers().into_iter().filter(|i| !edit.is_cut(t.words[*i].start, t.words[*i].end)).collect();
    let pauses: Vec<(f64, f64)> = t
        .pauses()
        .into_iter()
        .map(|(_, a, b)| (a + PAUSE_KEEP, b - PAUSE_KEEP))
        .filter(|(a, b)| b > a && edit.cut_at((a + b) * 0.5).is_none())
        .collect();
    let font = FontId::proportional(13.5);
    if !fillers.is_empty() {
        let label = format!("Remove {} filler word{}", fillers.len(), if fillers.len() == 1 { "" } else { "s" });
        let r = icons::button(ui, Icon::FillerWords, &label, font.clone(), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 36.0))
            .on_hover_text("Cut every detected filled pause (\"um\", \"uh\"…) out of the video");
        if r.clicked() {
            let spans: Vec<(f64, f64)> = fillers.iter().map(|i| word_span(t, *i, *i)).collect();
            removed(st, edit, &spans);
        }
    }
    if !pauses.is_empty() {
        let label = format!("Tighten {} pause{}", pauses.len(), if pauses.len() == 1 { "" } else { "s" });
        let r = icons::button(ui, Icon::TightenPauses, &label, font, theme::TEXT, Some(theme::SURFACE), vec2(0.0, 36.0))
            .on_hover_text("Shorten every long pause, keeping a natural breath on each side");
        if r.clicked() {
            removed(st, edit, &pauses);
        }
    }
    ui.add_space(10.0);
    document(ui, st, edit, t, time, &mut act);
    ui.add_space(16.0);
    help(ui, "Type to correct the text. Backspace cuts words out of the video, Ctrl+Backspace cuts them from the transcript only.");
    widgets::divider(ui);
    toggle_help(ui, "Show deleted text", "Keep cut words visible as strikethrough so they can be restored.", &mut edit.captions.show_deleted);
    act
}

/// Cuts `spans` out of the video and says how much went.
fn removed(st: &mut State, edit: &mut EditConfig, spans: &[(f64, f64)]) {
    let before = edit.cuts.iter().map(|c| c.end - c.start).sum::<f64>();
    for (a, b) in spans {
        edit.add_cut(*a, *b);
    }
    let gone = edit.cuts.iter().map(|c| c.end - c.start).sum::<f64>() - before;
    if gone > 0.0 {
        st.toast = Some(format!("Removed {gone:.1}s — Ctrl+Z to undo"));
    }
}

fn shown(edit: &EditConfig, t: &Transcript, i: usize) -> bool {
    let w = &t.words[i];
    edit.captions.show_deleted || !(edit.is_cut(w.start, w.end) || edit.captions.hidden_words.contains(&i))
}

/// Lays the transcript out as wrapped paragraphs from `origin`, `width` wide.
/// Returns the placed items, rows and total height.
fn layout(ui: &Ui, edit: &EditConfig, t: &Transcript, origin: Pos2, width: f32) -> (Vec<Placed>, Vec<Row>, f32) {
    let font = FontId::proportional(FONT);
    let painter = ui.painter();
    let gap = painter.layout_no_wrap(" ".into(), font.clone(), Color32::PLACEHOLDER).size().x;
    let pause_before: std::collections::HashMap<usize, (f64, f64)> = t.pauses().into_iter().map(|(i, a, b)| (i, (a, b))).collect();

    // Paragraphs: a new one after a long pause, or after a sentence and a breath.
    let mut paragraphs: Vec<Vec<Item>> = vec![];
    let mut prev: Option<usize> = None;
    for i in (0..t.words.len()).filter(|&i| shown(edit, t, i)) {
        let pause = pause_before.get(&i).copied();
        let sentence_break = prev.is_some_and(|p| {
            t.words[p].text.ends_with(['.', '?', '!', '؟']) && t.words[i].start - t.words[p].end >= 0.6
        });
        if prev.is_none() || pause.is_some() || sentence_break {
            paragraphs.push(vec![]);
            if let Some((a, b)) = pause.filter(|_| prev.is_some()) {
                paragraphs.last_mut().unwrap().push(Item::Pause(a, b));
            }
        }
        paragraphs.last_mut().unwrap().push(Item::Word(i));
        prev = Some(i);
    }

    let mut placed = vec![];
    let mut rows: Vec<Row> = vec![];
    let mut y = origin.y;
    for (pi, para) in paragraphs.iter().enumerate() {
        if pi > 0 {
            y += PARA_GAP;
        }
        let texts: Vec<&str> = para
            .iter()
            .map(|it| match it {
                Item::Word(i) => edit.captions.word_text(*i, &t.words[*i].text),
                Item::Pause(..) => "",
            })
            .collect();
        let rtl = bidi::paragraph_rtl(texts.iter().copied());
        let galleys: Vec<Arc<Galley>> = para
            .iter()
            .zip(&texts)
            .map(|(it, text)| match it {
                Item::Word(_) => painter.layout_no_wrap(text.to_string(), font.clone(), Color32::PLACEHOLDER),
                Item::Pause(a, b) => {
                    let tight = edit.cut_at((a + b) * 0.5).is_some();
                    let label = format!("··· {:.1}s", if tight { 2.0 * PAUSE_KEEP } else { b - a });
                    painter.layout_no_wrap(label, FontId::proportional(11.5), Color32::PLACEHOLDER)
                }
            })
            .collect();
        let item_w = |k: usize| match para[k] {
            Item::Word(_) => galleys[k].size().x,
            Item::Pause(..) => galleys[k].size().x + 14.0,
        };
        // Break lines in reading order, then set each line in display order.
        let mut lines: Vec<Vec<usize>> = vec![];
        let mut used = f32::INFINITY;
        for k in 0..para.len() {
            let w = item_w(k);
            if used + gap + w > width && !(used.is_infinite() && lines.is_empty() && k > 0) {
                lines.push(vec![]);
                used = -gap;
            }
            lines.last_mut().unwrap().push(k);
            used += gap + w;
        }
        for line in lines {
            let line_texts: Vec<&str> = line.iter().map(|&k| texts[k]).collect();
            let total: f32 = line.iter().map(|&k| item_w(k)).sum::<f32>() + gap * line.len().saturating_sub(1) as f32;
            let mut x = if rtl { origin.x + width - total } else { origin.x };
            for v in bidi::visual_order(&line_texts, rtl) {
                let k = line[v];
                let w = item_w(k);
                let h = if matches!(para[k], Item::Pause(..)) { 20.0 } else { galleys[k].size().y };
                let rect = Rect::from_min_size(pos2(x, y + (ROW_H - h) / 2.0), vec2(w, h));
                placed.push(Placed { item: para[k], rect, galley: galleys[k].clone(), row: rows.len() });
                x += w + gap;
            }
            rows.push(Row { top: y, rtl });
            y += ROW_H;
        }
    }
    (placed, rows, y - origin.y)
}

/// The word and character offset nearest `pos`.
fn hit(placed: &[Placed], rows: &[Row], pos: Pos2) -> Option<(usize, usize)> {
    let row = rows
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let d = |r: &Row| (pos.y - (r.top + ROW_H / 2.0)).abs();
            d(a).total_cmp(&d(b))
        })?
        .0;
    let (p, i) = placed
        .iter()
        .filter(|p| p.row == row)
        .filter_map(|p| match p.item {
            Item::Word(i) => Some((p, i)),
            Item::Pause(..) => None,
        })
        .min_by(|(a, _), (b, _)| {
            let d = |p: &Placed| if pos.x < p.rect.left() { p.rect.left() - pos.x } else { (pos.x - p.rect.right()).max(0.0) };
            d(a).total_cmp(&d(b))
        })?;
    let local = vec2((pos.x - p.rect.left()).clamp(0.0, p.rect.width()), p.galley.size().y / 2.0);
    Some((i, p.galley.cursor_from_pos(local).index.into()))
}

fn document(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, t: &Transcript, time: f64, act: &mut Action) {
    let width = ui.available_width();
    let origin = ui.cursor().min;
    let (placed, rows, height) = layout(ui, edit, t, origin, width);
    let rect = Rect::from_min_size(origin, vec2(width, height.max(ROW_H)));
    ui.allocate_rect(rect, Sense::hover());
    let id = doc_id();
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let focused = ui.memory(|m| m.has_focus(id));
    let shift = ui.input(|i| i.modifiers.shift);
    let word_at = |pos: Pos2| hit(&placed, &rows, pos);

    // Pause pills toggle tightening.
    let mut on_pill = false;
    if let Some(pos) = resp.hover_pos() {
        for p in &placed {
            if let Item::Pause(a, b) = p.item
                && p.rect.contains(pos)
            {
                on_pill = true;
                if resp.clicked() {
                    let tight = edit.cut_at((a + b) * 0.5).is_some();
                    if tight {
                        edit.restore(a, b);
                    } else {
                        removed(st, edit, &[(a + PAUSE_KEEP, b - PAUSE_KEEP)]);
                    }
                }
            }
        }
    }
    let doc = &mut st.doc;
    if on_pill {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // Mouse: drag selects words, click places the caret and seeks there.
    if resp.drag_started()
        && let Some((w, _)) = ui.input(|i| i.pointer.press_origin()).and_then(word_at)
    {
        doc.anchor = Some(w);
        doc.sel = Some((w, w));
        doc.caret = None;
        doc.dragging = true;
        resp.request_focus();
    }
    if doc.dragging
        && let (Some(a), Some((w, _))) = (doc.anchor, resp.interact_pointer_pos().and_then(word_at))
    {
        doc.sel = Some((a.min(w), a.max(w)));
    }
    if resp.drag_stopped() {
        doc.dragging = false;
    }
    if resp.clicked()
        && !on_pill
        && let Some((w, off)) = resp.interact_pointer_pos().and_then(word_at)
    {
        let from = doc.caret.map(|(c, _)| c).or(doc.anchor);
        match (shift, from) {
            (true, Some(a)) => {
                doc.sel = Some((a.min(w), a.max(w)));
                doc.caret = None;
            }
            _ => {
                doc.caret = Some((w, off));
                doc.sel = None;
                doc.anchor = Some(w);
                act.seek = Some(t.words[w].start);
            }
        }
        resp.request_focus();
    }
    if resp.double_clicked()
        && let Some((w, _)) = resp.interact_pointer_pos().and_then(word_at)
    {
        doc.sel = Some((w, w));
        doc.caret = None;
    }
    // A press anywhere else drops the selection (the menu included).
    let pressed_elsewhere = ui.input(|i| {
        i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !doc.menu.is_some_and(|m| m.contains(p)))
    });
    if pressed_elsewhere {
        doc.sel = None;
        doc.caret = None;
    }

    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false })
        });
        keyboard(ui, st, edit, t);
    }
    let doc = &st.doc;

    // Paint.
    let p = ui.painter();
    let selected = |i: usize| doc.sel.is_some_and(|(a, b)| i >= a && i <= b);
    let gap = p.layout_no_wrap(" ".into(), FontId::proportional(FONT), Color32::PLACEHOLDER).size().x;
    for pl in &placed {
        let Item::Word(i) = pl.item else { continue };
        if !selected(i) {
            continue;
        }
        // Join up with selected neighbours on the same row.
        let joins = |dx: f32| {
            placed.iter().any(|o| {
                o.row == pl.row && matches!(o.item, Item::Word(j) if selected(j)) && (if dx < 0.0 { (o.rect.right() + gap - pl.rect.left()).abs() < 1.0 } else { (pl.rect.right() + gap - o.rect.left()).abs() < 1.0 })
            })
        };
        let r = Rect::from_min_max(
            pos2(pl.rect.left() - if joins(-1.0) { gap / 2.0 + 0.5 } else { 1.0 }, rows[pl.row].top + 1.0),
            pos2(pl.rect.right() + if joins(1.0) { gap / 2.0 + 0.5 } else { 1.0 }, rows[pl.row].top + ROW_H - 1.0),
        );
        p.rect_filled(r, CornerRadius::same(2), SEL_BG);
    }
    for pl in &placed {
        match pl.item {
            Item::Pause(a, b) => {
                let tight = edit.cut_at((a + b) * 0.5).is_some();
                p.rect_filled(pl.rect, CornerRadius::same(6), PILL_BG);
                let g = pl.galley.clone();
                p.galley(pl.rect.center() - g.size() / 2.0, g, if tight { PILL_TIGHT } else { PILL_TEXT });
            }
            Item::Word(i) => {
                let w = &t.words[i];
                let cut = edit.is_cut(w.start, w.end) || edit.captions.hidden_words.contains(&i);
                let filler = transcript::is_filler(&w.text);
                let color = match (selected(i), cut, filler) {
                    (true, _, _) => SEL_TEXT,
                    (_, true, true) => FILLER_CUT,
                    (_, true, false) => DIM,
                    (_, false, true) => FILLER,
                    _ => theme::TEXT,
                };
                p.galley(pl.rect.min, pl.galley.clone(), color);
                let r = pl.rect;
                if cut {
                    let y = r.center().y + 0.5;
                    p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, color));
                } else if filler || transcript::is_soft_filler(&w.text) {
                    let c = if filler { FILLER } else { SOFT_UNDERLINE };
                    let y = r.bottom() - 1.0;
                    let mut x = r.left();
                    while x < r.right() {
                        p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(1.2, 1.2)), 0.0, c);
                        x += 2.6;
                    }
                }
            }
        }
    }
    // The playhead's line gets a dot in the margin.
    if let Some(cur) = t.words.iter().rposition(|w| w.start <= time)
        && let Some(pl) = placed.iter().find(|pl| matches!(pl.item, Item::Word(i) if i == cur))
    {
        let row = &rows[pl.row];
        let x = if row.rtl { rect.right() + 9.0 } else { rect.left() - 9.0 };
        let clip = ui.clip_rect().expand2(vec2(14.0, 0.0));
        p.with_clip_rect(clip).circle_filled(pos2(x, row.top + ROW_H / 2.0), 2.4, theme::TEXT);
    }
    // Blinking caret.
    if focused
        && doc.sel.is_none()
        && let Some((w, off)) = doc.caret
        && let Some(pl) = placed.iter().find(|pl| matches!(pl.item, Item::Word(i) if i == w))
    {
        let now = ui.input(|i| i.time);
        if (now * 2.0) as i64 % 2 == 0 {
            let x = pl.rect.left() + pl.galley.pos_from_cursor(egui::text::CCursor::new(off)).min.x;
            p.line_segment([pos2(x, pl.rect.top() + 2.0), pos2(x, pl.rect.bottom() - 2.0)], Stroke::new(1.5, theme::TEXT));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }

    // Menu beside the selection.
    st.doc.menu = None;
    if let Some((a, b)) = st.doc.sel
        && !st.doc.dragging
        && let Some(last) = placed.iter().filter(|pl| matches!(pl.item, Item::Word(i) if i >= a && i <= b)).max_by(|x, y| x.rect.top().total_cmp(&y.rect.top()))
    {
        let pos = pos2(rect.left() - 34.0 - MENU_W, last.rect.center().y - 40.0);
        menu(ui, st, edit, t, (a, b), pos, act);
    }
}

const MENU_W: f32 = 196.0;

fn menu(ui: &Ui, st: &mut State, edit: &mut EditConfig, t: &Transcript, (a, b): (usize, usize), pos: Pos2, act: &mut Action) {
    let all_cut = (a..=b).all(|i| edit.is_cut(t.words[i].start, t.words[i].end) || edit.captions.hidden_words.contains(&i));
    let (lo, hi) = word_span(t, a, b);
    let items: Vec<(Icon, &str, &str)> = if all_cut {
        vec![(Icon::Undo, "Restore", "Put these words back"), (Icon::Play, "Play", "Play from here")]
    } else {
        vec![
            (Icon::Scissors, "Cut video", "Cut this part out of the video"),
            (Icon::TranscriptLines, "Cut transcript", "Remove these words from the captions only"),
            (Icon::Play, "Play", "Play from here"),
        ]
    };
    let mut chosen = None;
    let area = egui::Area::new(doc_id().with("menu")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(Color32::from_rgb(0x24, 0x25, 0x2b))
            .stroke(Stroke::new(1.0, Color32::from_white_alpha(18)))
            .corner_radius(CornerRadius::same(14))
            .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(120) })
            .inner_margin(egui::Margin::same(7))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (icon, label, tip) in &items {
                    let (r, resp) = ui.allocate_exact_size(vec2(MENU_W - 14.0, 42.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(9), Color32::from_white_alpha(16));
                    }
                    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 24.0, r.center().y), vec2(19.0, 19.0)), *icon, theme::TEXT);
                    ui.painter().text(pos2(r.left() + 46.0, r.center().y), egui::Align2::LEFT_CENTER, *label, FontId::proportional(15.0), theme::TEXT);
                    if resp.on_hover_text(*tip).clicked() {
                        chosen = Some(*label);
                    }
                }
            });
    });
    st.doc.menu = Some(area.response.rect);
    match chosen {
        Some("Cut video") => {
            removed(st, edit, &[(lo, hi)]);
            st.doc.sel = None;
        }
        Some("Cut transcript") => {
            hide(edit, a, b);
            st.doc.sel = None;
        }
        Some("Restore") => {
            edit.restore(lo, hi);
            edit.captions.hidden_words.retain(|i| *i < a || *i > b);
            st.doc.sel = None;
        }
        Some("Play") => {
            act.seek = Some(t.words[a].start);
            st.play_request = true;
        }
        _ => {}
    }
}

/// Removes words `a..=b` from the captions only.
fn hide(edit: &mut EditConfig, a: usize, b: usize) {
    edit.captions.hidden_words.extend(a..=b);
    edit.captions.hidden_words.sort_unstable();
    edit.captions.hidden_words.dedup();
}

/// Sets word `i`'s shown text; an empty word drops out of the captions.
fn set_word(edit: &mut EditConfig, t: &Transcript, i: usize, text: String) {
    if text.trim().is_empty() {
        edit.captions.word_edits.remove(&i);
        hide(edit, i, i);
    } else if text == t.words[i].text {
        edit.captions.word_edits.remove(&i);
    } else {
        edit.captions.word_edits.insert(i, text);
    }
}

fn byte_at(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(b, _)| b)
}

/// Typing, deleting and caret movement while the transcript has focus.
fn keyboard(ui: &Ui, st: &mut State, edit: &mut EditConfig, t: &Transcript) {
    let events = ui.input(|i| i.events.clone());
    let visible: Vec<usize> = (0..t.words.len()).filter(|&i| shown(edit, t, i)).collect();
    let text_of = |edit: &EditConfig, i: usize| edit.captions.word_text(i, &t.words[i].text).to_string();
    for e in events {
        let doc = &mut st.doc;
        match e {
            Event::Text(s) | Event::Paste(s) => {
                if let Some((a, b)) = doc.sel.take() {
                    // Typing over a selection replaces those words' text.
                    let first = visible.iter().copied().find(|&i| i >= a && i <= b).unwrap_or(a);
                    set_word(edit, t, first, s.clone());
                    if b > first {
                        hide(edit, first + 1, b);
                    }
                    edit.captions.hidden_words.retain(|&i| i != first);
                    doc.caret = Some((first, s.chars().count()));
                } else if let Some((w, off)) = doc.caret {
                    let mut text = text_of(edit, w);
                    text.insert_str(byte_at(&text, off), &s);
                    set_word(edit, t, w, text);
                    doc.caret = Some((w, off + s.chars().count()));
                }
            }
            Event::Copy | Event::Cut => {
                if let Some((a, b)) = doc.sel {
                    let s: Vec<String> = (a..=b).filter(|i| visible.contains(i)).map(|i| text_of(edit, i)).collect();
                    ui.ctx().copy_text(s.join(" "));
                }
            }
            Event::Key { key, pressed: true, modifiers, .. } => match key {
                Key::Backspace | Key::Delete => {
                    if let Some((a, b)) = doc.sel {
                        let all_cut = (a..=b).all(|i| edit.is_cut(t.words[i].start, t.words[i].end) || edit.captions.hidden_words.contains(&i));
                        if modifiers.command {
                            hide(edit, a, b);
                        } else if !all_cut {
                            let span = word_span(t, a, b);
                            removed(st, edit, &[span]);
                        }
                        st.doc.sel = None;
                    } else if let Some((w, off)) = doc.caret {
                        let text = text_of(edit, w);
                        let len = text.chars().count();
                        let del = if key == Key::Backspace { off.checked_sub(1) } else { (off < len).then_some(off) };
                        match del {
                            Some(d) => {
                                let mut s = text;
                                s.replace_range(byte_at(&s, d)..byte_at(&s, d + 1), "");
                                let empty = s.trim().is_empty();
                                set_word(edit, t, w, s);
                                doc.caret = if empty {
                                    visible.iter().rev().find(|&&i| i < w).map(|&i| (i, text_of(edit, i).chars().count()))
                                } else {
                                    Some((w, d))
                                };
                            }
                            // At the start of a word, Backspace steps into the previous one.
                            None if key == Key::Backspace => {
                                if let Some(&p) = visible.iter().rev().find(|&&i| i < w) {
                                    doc.caret = Some((p, text_of(edit, p).chars().count()));
                                }
                            }
                            None => {}
                        }
                    }
                }
                Key::ArrowLeft | Key::ArrowRight => {
                    let Some((w, off)) = doc.caret.or(doc.sel.map(|(a, _)| (a, 0))) else { continue };
                    doc.sel = None;
                    // In right-to-left text the arrows run the other way.
                    let forward = (key == Key::ArrowRight) != bidi::is_rtl(&text_of(edit, w));
                    let len = text_of(edit, w).chars().count();
                    doc.caret = Some(if forward {
                        if off < len {
                            (w, off + 1)
                        } else {
                            visible.iter().find(|&&i| i > w).map_or((w, off), |&n| (n, 0))
                        }
                    } else if off > 0 {
                        (w, off - 1)
                    } else {
                        visible.iter().rev().find(|&&i| i < w).map_or((w, off), |&p| (p, text_of(edit, p).chars().count()))
                    });
                }
                Key::A if modifiers.command => {
                    if let (Some(&a), Some(&b)) = (visible.first(), visible.last()) {
                        doc.sel = Some((a, b));
                        doc.caret = None;
                    }
                }
                Key::Escape => {
                    doc.sel = None;
                    doc.caret = None;
                    ui.memory_mut(|m| m.surrender_focus(doc_id()));
                }
                _ => {}
            },
            _ => {}
        }
    }
}
