//! Cursor style picker: the built-in vector set, imported cursor themes and
//! themes installed on the system.

use super::widgets;
use crate::cursor_themes::{self, Theme};
use crate::dialogs;
use crate::edit::EditConfig;
use crate::icons::{self, Icon};
use crate::theme;
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Ui, vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Default)]
pub struct State {
    themes: Option<Vec<Theme>>,
    previews: HashMap<String, Option<egui::TextureHandle>>,
    picking: Option<Receiver<Vec<PathBuf>>>,
    importing: Option<Receiver<Result<Theme, String>>>,
    message: Option<String>,
}

const BUILTIN: &str = "builtin";

fn preview(ctx: &egui::Context, cache: &mut HashMap<String, Option<egui::TextureHandle>>, key: &str, dir: Option<&str>) -> Option<egui::TextureHandle> {
    cache
        .entry(key.to_string())
        .or_insert_with(|| {
            let (rgba, w, h) = match dir {
                None => {
                    let v = crate::render::cursor_image::Vector::Arrow;
                    let n = crate::render::cursor_image::CANVAS;
                    (crate::render::cursor_image::rasterize(v), n, n)
                }
                Some(d) => {
                    let img = crate::xcursor::load_from_dir(std::path::Path::new(d), "default", 96)?;
                    (img.rgba, img.width, img.height)
                }
            };
            let img = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba);
            Some(ctx.load_texture(format!("cursor-preview-{key}"), img, egui::TextureOptions::LINEAR))
        })
        .clone()
}

fn tile(ui: &mut Ui, tex: Option<&egui::TextureHandle>, label: &str, selected: bool, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size + 18.0), Sense::click());
    let box_r = Rect::from_min_size(rect.min, vec2(size, size));
    let fill = if selected {
        theme::SURFACE_HOVER
    } else if resp.hovered() {
        Color32::from_rgb(0x21, 0x23, 0x28)
    } else {
        theme::SURFACE
    };
    let p = ui.painter();
    // Light backdrop so dark cursors are visible.
    p.rect_filled(box_r, CornerRadius::same(8), fill);
    p.rect_filled(box_r.shrink(6.0), CornerRadius::same(5), Color32::from_rgb(0x9a, 0x9c, 0xa4));
    if selected {
        p.rect_stroke(box_r, CornerRadius::same(8), egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
    }
    if let Some(t) = tex {
        let [w, h] = t.size();
        let s = (box_r.width() - 16.0) / w.max(h) as f32;
        let r = Rect::from_center_size(box_r.center(), vec2(w as f32 * s, h as f32 * s));
        egui::Image::new((t.id(), r.size())).paint_at(ui, r);
    }
    let mut text = label.to_string();
    if text.chars().count() > 11 {
        text = text.chars().take(10).collect::<String>() + "…";
    }
    ui.painter().text(
        egui::pos2(box_r.center().x, box_r.bottom() + 10.0),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.5),
        if selected { theme::TEXT } else { theme::TEXT_DIM },
    );
    resp.on_hover_text(label).on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn style_picker(ui: &mut Ui, st: &mut State, edit: &mut EditConfig) {
    let ctx = ui.ctx().clone();
    // Results of the file dialog and of the import worker.
    if let Some(rx) = &st.picking {
        match rx.try_recv() {
            Ok(files) => {
                st.picking = None;
                if let Some(f) = files.into_iter().next() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(cursor_themes::import(&f).map_err(|e| format!("{e:#}")));
                        c.request_repaint();
                    });
                    st.importing = Some(rx);
                }
            }
            Err(TryRecvError::Disconnected) => st.picking = None,
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    if let Some(rx) = &st.importing {
        match rx.try_recv() {
            Ok(Ok(t)) => {
                edit.cursor.theme = Some(t.dir.to_string_lossy().into_owned());
                st.message = Some(format!("Imported {}.", t.name));
                st.themes = None;
                st.importing = None;
            }
            Ok(Err(e)) => {
                st.message = Some(e);
                st.importing = None;
            }
            Err(TryRecvError::Disconnected) => st.importing = None,
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    let themes = st.themes.get_or_insert_with(cursor_themes::list).clone();

    theme::section(ui, "Cursor style");
    let size = ((ui.available_width() - 3.0 * 8.0) / 4.0).floor().min(78.0);
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let tex = preview(&ctx, &mut st.previews, BUILTIN, None);
        if tile(ui, tex.as_ref(), "Open Studio", edit.cursor.theme.is_none(), size).clicked() {
            edit.cursor.theme = None;
        }
        for t in &themes {
            let dir = t.dir.to_string_lossy().into_owned();
            let tex = preview(&ctx, &mut st.previews, &dir, Some(&dir));
            let selected = edit.cursor.theme.as_deref() == Some(dir.as_str());
            let resp = tile(ui, tex.as_ref(), &t.name, selected, size);
            if resp.clicked() {
                edit.cursor.theme = Some(dir.clone());
            }
            if t.imported {
                resp.context_menu(|ui| {
                    if ui.button("Remove theme").clicked() {
                        remove = Some(t.clone());
                        ui.close();
                    }
                });
            }
        }
    });
    if let Some(t) = remove {
        if edit.cursor.theme.as_deref() == Some(&*t.dir.to_string_lossy()) {
            edit.cursor.theme = None;
        }
        let _ = cursor_themes::remove(&t);
        st.themes = None;
    }
    ui.add_space(4.0);
    let busy = st.picking.is_some() || st.importing.is_some();
    ui.horizontal(|ui| {
        let half = (ui.available_width() - 8.0) / 2.0;
        let font = egui::FontId::proportional(13.0);
        if icons::button(ui, Icon::Plus, "Import archive", font.clone(), theme::TEXT, Some(theme::SURFACE), vec2(half, 34.0)).clicked() && !busy {
            st.message = None;
            st.picking = Some(dialogs::open_files(ctx.clone(), "Import cursor theme archive", None, false));
        }
        if icons::button(ui, Icon::Folder, "Import folder", font, theme::TEXT, Some(theme::SURFACE), vec2(half, 34.0)).clicked() && !busy {
            st.message = None;
            st.picking = Some(dialogs::open_folder(ctx.clone(), "Import cursor theme folder", None));
        }
    });
    if busy {
        ui.add(egui::Spinner::new());
    }
    let msg = st.message.clone().unwrap_or_else(|| {
        "XCursor themes, e.g. from KDE Store or GitHub, as an archive or a folder. Right-click an imported theme to remove it.".into()
    });
    ui.label(RichText::new(msg).size(12.0).color(theme::TEXT_DIM));
    widgets::divider(ui);
}
