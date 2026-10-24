//! Shortcuts Guide (Ctrl+/), as in Screen Studio: every editor shortcut in
//! one window, grouped, with a filter field and category chips.

use super::Editor;
use crate::icons::{self, Icon};
use crate::theme;
use egui::{Color32, CornerRadius, FontId, Key, Sense, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Category {
    Editor,
    General,
    Layout,
    Project,
    Slice,
}

impl Category {
    pub const ALL: [Category; 5] = [Category::Editor, Category::General, Category::Layout, Category::Project, Category::Slice];

    fn name(self) -> &'static str {
        match self {
            Category::Editor => "Editor",
            Category::General => "General",
            Category::Layout => "Layout",
            Category::Project => "Project",
            Category::Slice => "Slice",
        }
    }
}

pub struct Shortcut {
    pub category: Category,
    /// Heading in the list ("Playback", "Clipboard").
    pub group: &'static str,
    pub keys: &'static [&'static str],
    pub label: &'static str,
}

const fn s(category: Category, group: &'static str, keys: &'static [&'static str], label: &'static str) -> Shortcut {
    Shortcut { category, group, keys, label }
}

use Category::*;

/// What the editor does with each key. Keep in step with `handle_keys`.
pub const SHORTCUTS: &[Shortcut] = &[
    s(General, "Application", &["Ctrl", "K"], "Open command menu"),
    s(General, "Application", &["Ctrl", "/"], "Show shortcuts guide"),
    s(General, "Application", &["Ctrl", ","], "Show settings"),
    s(General, "View", &["Ctrl", "="], "Zoom in preview"),
    s(General, "View", &["Ctrl", "-"], "Zoom out preview"),
    s(General, "View", &["Ctrl", "1…9"], "Show sidebar tab"),
    s(Editor, "Clipboard", &["Ctrl", "C"], "Copy current frame as image"),
    s(Editor, "Editor", &["Ctrl", "Shift", "Z"], "Redo"),
    s(Editor, "Editor", &["Ctrl", "Z"], "Undo"),
    s(Editor, "Editor", &["Shift", "C"], "Crop recording"),
    s(Editor, "Editor", &["Esc"], "Deselect"),
    s(Editor, "Playback", &["Space"], "Play / pause"),
    s(Editor, "Playback", &["K"], "Play / pause"),
    s(Editor, "Playback", &["L"], "Play forwards, faster on each press (up to 16×)"),
    s(Editor, "Playback", &["J"], "Play backwards, faster on each press (up to 16×)"),
    s(Editor, "Playback", &["K", "L"], "Next frame (hold K)"),
    s(Editor, "Playback", &["K", "J"], "Previous frame (hold K)"),
    s(Editor, "Playback", &["→"], "Next frame"),
    s(Editor, "Playback", &["←"], "Previous frame"),
    s(Slice, "Slice", &["C"], "Split clip at current time"),
    s(Slice, "Slice", &["Alt", "Click"], "Split clip or zoom where clicked"),
    s(Slice, "Slice", &["C"], "Split selected zoom at current time"),
    s(Slice, "Slice", &["Delete"], "Remove selected part"),
    s(Layout, "Layouts", &["Alt", "B"], "Add both layout"),
    s(Layout, "Layouts", &["Alt", "L"], "Add camera layout"),
    s(Layout, "Layouts", &["Alt", "S"], "Add screen layout"),
    s(Project, "Project", &["Ctrl", "S"], "Save project"),
    s(Project, "Project", &["Ctrl", "Shift", "S"], "Save project as…"),
    s(Project, "Export", &["Ctrl", "Shift", "E"], "Export…"),
];

/// Shortcuts in `category` (all when None) whose label, group or keys hold
/// every word of `query`, grouped under their headings in list order.
pub fn matching(query: &str, category: Option<Category>) -> Vec<(&'static str, Vec<&'static Shortcut>)> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut out: Vec<(&'static str, Vec<&'static Shortcut>)> = vec![];
    for sc in SHORTCUTS {
        if category.is_some_and(|c| c != sc.category) {
            continue;
        }
        let hay = format!("{} {} {}", sc.group, sc.label, sc.keys.join(" ")).to_lowercase();
        if !words.iter().all(|w| hay.contains(w.as_str())) {
            continue;
        }
        match out.iter_mut().find(|(g, _)| *g == sc.group) {
            Some((_, v)) => v.push(sc),
            None => out.push((sc.group, vec![sc])),
        }
    }
    out
}

#[derive(Default)]
pub struct Guide {
    query: String,
    category: Option<Category>,
    focused: bool,
}

/// Draws a row of keycaps right-aligned at `right`.
fn keycaps(p: &egui::Painter, right: egui::Pos2, keys: &[&str]) {
    let font = FontId::proportional(12.5);
    let mut x = right.x;
    for k in keys.iter().rev() {
        let g = p.layout_no_wrap(k.to_string(), font.clone(), theme::TEXT);
        let w = (g.size().x + 14.0).max(24.0);
        let r = egui::Rect::from_min_max(pos2(x - w, right.y - 12.0), pos2(x, right.y + 12.0));
        p.rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(14));
        p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, Color32::from_white_alpha(22)), egui::StrokeKind::Inside);
        p.galley(r.center() - g.size() / 2.0, g, theme::TEXT);
        x -= w + 4.0;
    }
}

impl Editor {
    pub(super) fn open_guide(&mut self) {
        self.guide = Some(Guide::default());
    }

    /// Draws the Shortcuts Guide while it's open.
    pub(super) fn guide_window(&mut self, ctx: &egui::Context) {
        let Some(guide) = self.guide.as_mut() else { return };
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape) || i.consume_key(egui::Modifiers::COMMAND, Key::Slash));
        let mut close = esc;
        let screen = ctx.content_rect();
        let dim = egui::Area::new(egui::Id::NULL.with("guide-dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(130));
            resp.clicked()
        });
        close |= dim.inner;
        let w = (screen.width() - 80.0).min(620.0);
        let h = (screen.height() - 80.0).min(640.0);
        egui::Area::new(egui::Id::NULL.with("guide"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos2(screen.center().x - w / 2.0, screen.center().y - h / 2.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x1f, 0x1f, 0x21))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::epaint::Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(150) })
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.set_height(h);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // Title bar: keyboard icon and name, close button.
                        let (bar, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
                        let p = ui.painter();
                        let title = p.layout_no_wrap("Shortcuts Guide".into(), theme::semibold(14.0), theme::TEXT);
                        let tw = title.size().x + 24.0;
                        let left = bar.center().x - tw / 2.0;
                        icons::paint(p, egui::Rect::from_center_size(pos2(left + 8.0, bar.center().y), vec2(16.0, 16.0)), Icon::Keyboard, theme::TEXT);
                        p.galley(pos2(left + 24.0, bar.center().y - title.size().y / 2.0), title, theme::TEXT);
                        let xr = egui::Rect::from_center_size(pos2(bar.right() - 24.0, bar.center().y), vec2(26.0, 26.0));
                        let x = ui.interact(xr, ui.scope_id().with("guide-close"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        let col = if x.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
                        icons::paint(ui.painter(), xr.shrink(7.0), Icon::Close, col);
                        close |= x.clicked();
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            egui::Frame::new().fill(Color32::from_white_alpha(10)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut guide.query)
                                        .hint_text(egui::RichText::new("Filter shortcuts...").color(Color32::from_rgb(0x7a, 0x7b, 0x80)))
                                        .font(FontId::proportional(14.0))
                                        .frame(egui::Frame::NONE)
                                        .desired_width(w - 36.0 - 20.0),
                                );
                                if !guide.focused {
                                    r.request_focus();
                                    guide.focused = true;
                                }
                            });
                        });
                        ui.add_space(10.0);
                        // Category chips.
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let chips = std::iter::once(None).chain(Category::ALL.into_iter().map(Some));
                            for c in chips {
                                let name = c.map_or("All", Category::name);
                                let on = guide.category == c;
                                let g = ui.painter().layout_no_wrap(name.into(), FontId::proportional(12.5), theme::TEXT);
                                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 26.0), Sense::click());
                                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                                let fill = if on {
                                    Color32::from_rgba_unmultiplied(0x5b, 0x45, 0xf0, 110)
                                } else if resp.hovered() {
                                    Color32::from_white_alpha(20)
                                } else {
                                    Color32::from_white_alpha(9)
                                };
                                ui.painter().rect_filled(r, CornerRadius::same(13), fill);
                                ui.painter().galley(r.center() - g.size() / 2.0, g, if on { Color32::WHITE } else { theme::TEXT });
                                if resp.clicked() {
                                    guide.category = c;
                                }
                            }
                        });
                        ui.add_space(10.0);
                        let (line, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                        ui.painter().rect_filled(line, 0.0, Color32::from_white_alpha(22));
                        let groups = matching(&guide.query, guide.category);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.add_space(6.0);
                            if groups.is_empty() {
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("No matching shortcuts").size(14.0).color(theme::TEXT_DIM));
                                });
                            }
                            for (group, items) in groups {
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new(group).size(13.0).color(Color32::from_rgb(0x8e, 0x8f, 0x94)));
                                });
                                ui.add_space(4.0);
                                for sc in items {
                                    let (r, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
                                    let p = ui.painter();
                                    p.text(pos2(r.left() + 20.0, r.center().y), egui::Align2::LEFT_CENTER, sc.label, FontId::proportional(13.5), theme::TEXT);
                                    keycaps(p, pos2(r.right() - 20.0, r.center().y), sc.keys);
                                }
                            }
                            ui.add_space(12.0);
                        });
                    });
            });
        if close {
            self.guide = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_by_words_and_category() {
        // Everything, grouped in list order, each shortcut once.
        let all = matching("", None);
        assert_eq!(all.iter().map(|(_, v)| v.len()).sum::<usize>(), SHORTCUTS.len());
        assert_eq!(all[0].0, "Application");
        // Words match label, group or keys, all of them.
        let copy = matching("copy frame", None);
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].1[0].keys, ["Ctrl", "C"]);
        assert!(matching("ctrl shift", None).iter().flat_map(|(_, v)| v).all(|s| s.keys.contains(&"Shift")));
        assert!(matching("PLAYBACK backwards", None)[0].1.iter().any(|s| s.keys == ["J"]));
        // A chip shows only its category.
        let slice = matching("", Some(Category::Slice));
        assert!(slice.iter().flat_map(|(_, v)| v).all(|s| s.category == Category::Slice));
        assert!(matching("save", Some(Category::Slice)).is_empty());
        // Every category has something to show.
        for c in Category::ALL {
            assert!(!matching("", Some(c)).is_empty(), "{c:?} is empty");
        }
    }
}
