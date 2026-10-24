//! The top bar's Command Menu (Ctrl+K, the button with the command glyph)
//! and Presets dropdown, after Screen Studio's, plus the file jobs they start
//! (frame images, extracted recording files).

use super::sidebar::{self, Tab};
use super::{Editor, ExportDialog, spawn_export};
use crate::dialogs;
use crate::icons::{self, Icon};
use crate::presets::{self, Preset};
use crate::settings::{Destination, Format, Settings};
use crate::theme;
use egui::{Color32, CornerRadius, FontId, Key, Rect, Sense, Stroke, pos2, vec2};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct CommandMenu {
    query: String,
    /// Highlighted row among the filtered commands.
    sel: usize,
    focused: bool,
}

/// Work waiting on a file dialog.
pub(super) enum FileTask {
    /// Save the frame at this recording time as an image.
    Frame(f64),
    /// Copy this project file.
    Extract(PathBuf),
    /// Copy every recorded file into a folder.
    ExtractAll,
}

#[derive(Clone)]
enum Cmd {
    Export,
    Frame,
    CopyFrame,
    ExtractAll,
    Extract(PathBuf, &'static str),
    ExportClipboard,
    ExportFile,
    Save,
    SaveAs,
    ShowFolder,
    Undo,
    Redo,
    Crop,
    Split,
    Quality(u32),
    Tab(Tab),
    Preset(usize),
    ToggleLastSettings,
}

struct Entry {
    group: &'static str,
    icon: Icon,
    label: String,
    /// Dim text after the label ("with current settings").
    note: &'static str,
    cmd: Cmd,
}

impl Editor {
    fn commands(&self) -> Vec<Entry> {
        let e = |group, icon, label: &str, note, cmd| Entry { group, icon, label: label.to_string(), note, cmd };
        let paths = crate::project::ProjectPaths::new(&self.dir);
        let mut out = vec![
            e("Export", Icon::ExportFilled, "Export…", "", Cmd::Export),
            e("Export", Icon::Export, "Export current frame as image", "", Cmd::Frame),
            e("Export", Icon::Clipboard, "Copy current frame as image", "Ctrl+C", Cmd::CopyFrame),
            e("Export", Icon::ArrowRight, "Extract recording files", "", Cmd::ExtractAll),
            e("Export", Icon::ExtractFile, "Extract screen recording file", "", Cmd::Extract(paths.video(), "screen")),
        ];
        if let Some(c) = &self.rec.camera {
            out.push(e("Export", Icon::ExtractFile, "Extract camera video file", "", Cmd::Extract(self.dir.join(&c.file), "camera")));
        }
        if let Some(a) = self.rec.audio.iter().find(|a| a.kind == "microphone") {
            out.push(e("Export", Icon::ExtractFile, "Extract microphone audio file", "", Cmd::Extract(self.dir.join(&a.file), "microphone")));
        }
        out.push(e("Export", Icon::ClipboardVideo, "Export to clipboard", "with current settings", Cmd::ExportClipboard));
        out.push(e("Export", Icon::ExportFilled, "Export to file…", "with current settings", Cmd::ExportFile));
        out.push(e("Project", Icon::Folder, "Save project", "Ctrl+S", Cmd::Save));
        out.push(e("Project", Icon::Folder, "Save project as…", "Ctrl+Shift+S", Cmd::SaveAs));
        out.push(e("Project", Icon::ArrowRight, "Show project in folder", "", Cmd::ShowFolder));
        out.push(e("Edit", Icon::Undo, "Undo", "Ctrl+Z", Cmd::Undo));
        out.push(e("Edit", Icon::Redo, "Redo", "Ctrl+Shift+Z", Cmd::Redo));
        out.push(e("Edit", Icon::Scissors, "Split clip at current time", "C", Cmd::Split));
        out.push(e("Edit", Icon::Crop, "Crop recording", "Shift+C", Cmd::Crop));
        for (i, p) in self.presets.iter().enumerate() {
            out.push(Entry { group: "Presets", icon: Icon::Presets, label: format!("Apply preset: {}", p.name), note: "", cmd: Cmd::Preset(i) });
        }
        let on = Settings::load().use_last_project_settings;
        out.push(Entry {
            group: "Settings",
            icon: Icon::Presets,
            label: format!("{} last project settings as default for new recordings", if on { "Stop using" } else { "Use" }),
            note: "",
            cmd: Cmd::ToggleLastSettings,
        });
        for (d, name) in [(1, "Full"), (2, "Half"), (4, "Quarter")] {
            out.push(Entry { group: "View", icon: Icon::Quality, label: format!("Preview quality: {name}"), note: "", cmd: Cmd::Quality(d) });
        }
        for (n, (tab, icon, name)) in sidebar::TABS.iter().enumerate() {
            if sidebar::tab_enabled(*tab, &self.rec) {
                let note = ["Ctrl+1", "Ctrl+2", "Ctrl+3", "Ctrl+4", "Ctrl+5", "Ctrl+6", "Ctrl+7", "Ctrl+8", "Ctrl+9"][n];
                out.push(Entry { group: "View", icon: *icon, label: format!("Show {name}"), note, cmd: Cmd::Tab(*tab) });
            }
        }
        out
    }

    /// Copies the edited frame under the playhead to the clipboard as a PNG,
    /// at export size (Ctrl+C, or the preview's right-click menu).
    pub(super) fn copy_frame(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.file_done = Some(rx);
        let (dir, rec, edit, t) = (self.dir.clone(), self.rec.clone(), self.edit.clone(), self.time);
        let (device, queue) = (self.comp.device().clone(), self.comp.queue().clone());
        let height = Settings::load().height.unwrap_or(self.rec.height.clamp(720, 2160));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut comp = crate::render::Compositor::new(device, queue);
            let res = crate::export::frame_png(&mut comp, &dir, &rec, &edit, t, height).map_err(|e| format!("{e:#}"));
            let msg = match res.and_then(|png| crate::clipboard::copy("image/png", &png)) {
                Ok(()) => "Frame copied: paste it anywhere as an image".to_string(),
                Err(e) => format!("Couldn't copy the frame: {e}"),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub(super) fn open_command_menu(&mut self) {
        self.presets = presets::list();
        self.command = Some(CommandMenu::default());
    }

    /// Draws the command menu when it's open and runs the chosen command.
    pub(super) fn command_menu(&mut self, ctx: &egui::Context) {
        if self.command.is_none() {
            return;
        }
        let all = self.commands();
        let menu = self.command.as_mut().unwrap();
        let words: Vec<String> = menu.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        let shown: Vec<&Entry> = all
            .iter()
            .filter(|c| {
                let hay = format!("{} {} {}", c.group, c.label, c.note).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if down && menu.sel + 1 < shown.len() {
            menu.sel += 1;
        }
        if up {
            menu.sel = menu.sel.saturating_sub(1);
        }
        menu.sel = menu.sel.min(shown.len().saturating_sub(1));
        let mut run: Option<Cmd> = enter.then(|| shown.get(menu.sel).map(|c| c.cmd.clone())).flatten();
        let mut close = esc;

        // Dim the editor; a click on it closes the menu.
        let screen = ctx.content_rect();
        let dim = egui::Area::new(egui::Id::NULL.with("command-dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(130));
            resp.clicked()
        });
        close |= dim.inner;

        let w = (screen.width() - 80.0).min(760.0);
        egui::Area::new(egui::Id::NULL.with("command-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos2(screen.center().x - w / 2.0, screen.top() + screen.height() * 0.16))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x1f, 0x1f, 0x21))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(egui::epaint::Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(150) })
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            let edit = egui::TextEdit::singleline(&mut menu.query)
                                .hint_text(egui::RichText::new("Type to find a command").color(Color32::from_rgb(0x7a, 0x7b, 0x80)))
                                .font(FontId::proportional(19.0))
                                .frame(egui::Frame::NONE)
                                .desired_width(w - 36.0);
                            let r = ui.add(edit);
                            if !menu.focused {
                                r.request_focus();
                                menu.focused = true;
                            }
                            if r.changed() {
                                menu.sel = 0;
                            }
                        });
                        ui.add_space(14.0);
                        let (line, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                        ui.painter().rect_filled(line, 0.0, Color32::from_white_alpha(22));
                        egui::ScrollArea::vertical().max_height(screen.height() * 0.5).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add_space(8.0);
                            if shown.is_empty() {
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("No matching commands").size(14.0).color(theme::TEXT_DIM));
                                });
                                ui.add_space(14.0);
                            }
                            let mut group = "";
                            for (n, c) in shown.iter().enumerate() {
                                if c.group != group {
                                    group = c.group;
                                    ui.add_space(if n == 0 { 6.0 } else { 12.0 });
                                    ui.horizontal(|ui| {
                                        ui.add_space(20.0);
                                        ui.label(egui::RichText::new(group).size(13.0).color(Color32::from_rgb(0x8e, 0x8f, 0x94)));
                                    });
                                    ui.add_space(6.0);
                                }
                                let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
                                if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                    menu.sel = n;
                                }
                                let p = ui.painter();
                                if n == menu.sel {
                                    p.rect_filled(r.shrink2(vec2(6.0, 1.0)), CornerRadius::same(7), Color32::from_white_alpha(14));
                                    if down || up {
                                        ui.scroll_to_rect(r, None);
                                    }
                                }
                                icons::paint(p, Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(18.0, 18.0)), c.icon, theme::TEXT);
                                let g = p.layout_no_wrap(c.label.clone(), FontId::proportional(15.0), theme::TEXT);
                                let gw = g.size().x;
                                p.galley(pos2(r.left() + 54.0, r.center().y - g.size().y / 2.0), g, theme::TEXT);
                                if !c.note.is_empty() {
                                    p.text(pos2(r.left() + 54.0 + gw + 10.0, r.center().y), egui::Align2::LEFT_CENTER, c.note, FontId::proportional(14.0), Color32::from_rgb(0x84, 0x85, 0x8b));
                                }
                                if resp.clicked() {
                                    run = Some(c.cmd.clone());
                                }
                            }
                            ui.add_space(8.0);
                        });
                    });
            });
        if let Some(cmd) = run {
            self.command = None;
            self.run_command(cmd, ctx);
        } else if close {
            self.command = None;
        }
    }

    fn run_command(&mut self, cmd: Cmd, ctx: &egui::Context) {
        let settings = Settings::load();
        let dir = settings.export_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        match cmd {
            Cmd::Export => {
                if self.export.is_none() {
                    self.export = Some(ExportDialog { settings, job: None, picking_dir: None, saving: None });
                }
            }
            Cmd::Frame => {
                let secs = self.time.max(0.0);
                let name = format!("{} {:02}.{:02}.{:02}.png", self.name, (secs / 60.0) as u32, secs as u32 % 60, ((secs.fract()) * 100.0) as u32);
                self.file_task = Some((dialogs::save_file(ctx.clone(), "Export current frame as image", &dir, &name), FileTask::Frame(self.time)));
            }
            Cmd::CopyFrame => self.copy_frame(ctx),
            Cmd::ExtractAll => {
                self.file_task = Some((dialogs::open_folder(ctx.clone(), "Extract recording files to", Some(dir)), FileTask::ExtractAll));
            }
            Cmd::Extract(src, what) => {
                let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
                let name = format!("{} {what}.{ext}", self.name);
                self.file_task = Some((dialogs::save_file(ctx.clone(), "Extract file", &dir, &name), FileTask::Extract(src)));
            }
            Cmd::ExportClipboard | Cmd::ExportFile => {
                if self.export.as_ref().is_some_and(|d| d.job.is_some()) {
                    return;
                }
                let mut dlg = ExportDialog { settings, job: None, picking_dir: None, saving: None };
                let ext = if dlg.settings.format == Format::Gif { "gif" } else { "mp4" };
                let name = format!("{}.{ext}", self.name);
                if matches!(cmd, Cmd::ExportClipboard) {
                    dlg.settings.destination = Destination::Clipboard;
                    let tmp = std::env::temp_dir().join("open-studio-exports");
                    let _ = std::fs::create_dir_all(&tmp);
                    let height = dlg.settings.height.unwrap_or(self.rec.height.clamp(720, 2160));
                    spawn_export(&mut dlg, tmp.join(name), height, &self.dir, &self.rec, &self.edit, &self.comp, ctx);
                } else {
                    dlg.settings.destination = Destination::File;
                    dlg.saving = Some(dialogs::save_file(ctx.clone(), "Export video", &dir, &name));
                }
                self.export = Some(dlg);
            }
            Cmd::Save => self.save_with_feedback(),
            Cmd::SaveAs => {
                if self.saving_as.is_none() {
                    self.saving_as = Some(dialogs::open_folder(ctx.clone(), "Save project to", Some(crate::util::projects_dir())));
                }
            }
            Cmd::ShowFolder => {
                let _ = std::process::Command::new("xdg-open").arg(&self.dir).spawn();
            }
            Cmd::Undo => self.undo(),
            Cmd::Redo => self.redo(),
            Cmd::Crop => self.open_crop_tool(),
            Cmd::Split => {
                self.edit.split_at(self.time, self.rec.duration);
            }
            Cmd::Quality(d) => {
                self.preview_quality = d;
                let mut s = Settings::load();
                s.preview_quality = d;
                s.save();
            }
            Cmd::Tab(t) => self.tab = t,
            Cmd::ToggleLastSettings => {
                let mut s = Settings::load();
                s.use_last_project_settings = !s.use_last_project_settings;
                s.save();
                self.notify(if s.use_last_project_settings {
                    "New recordings will use the last project's settings"
                } else {
                    "New recordings will start with the default settings"
                });
            }
            Cmd::Preset(i) => {
                if let Some(p) = self.presets.get(i) {
                    self.edit.apply_preset(&p.settings);
                }
            }
        }
    }

    /// Finishes file jobs once their dialog returns, and reports results.
    pub(super) fn poll_file_tasks(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.file_done {
            match rx.try_recv() {
                Ok(msg) => {
                    self.file_done = None;
                    self.notify(msg);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.file_done = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        let Some((rx, _)) = &self.file_task else { return };
        let picked = match rx.try_recv() {
            Ok(p) => p,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Err(_) => vec![],
        };
        let (_, task) = self.file_task.take().unwrap();
        let Some(target) = picked.into_iter().next() else { return };
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.file_done = Some(rx);
        let ctx = ctx.clone();
        match task {
            FileTask::Frame(t) => {
                let mut out = target;
                if out.extension().is_none_or(|e| !e.eq_ignore_ascii_case("png")) {
                    out.set_extension("png");
                }
                let (dir, rec, edit) = (self.dir.clone(), self.rec.clone(), self.edit.clone());
                let (device, queue) = (self.comp.device().clone(), self.comp.queue().clone());
                let height = Settings::load().height.unwrap_or(self.rec.height.clamp(720, 2160));
                std::thread::spawn(move || {
                    let mut comp = crate::render::Compositor::new(device, queue);
                    let msg = match crate::export::export_frame(&mut comp, &dir, &rec, &edit, t, height, &out) {
                        Ok(()) => format!("Frame saved: {}", crate::util::tilde(&out)),
                        Err(e) => format!("Couldn't save the frame: {e:#}"),
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
            FileTask::Extract(src) => {
                std::thread::spawn(move || {
                    let msg = match std::fs::copy(&src, &target) {
                        Ok(_) => format!("File extracted: {}", crate::util::tilde(&target)),
                        Err(e) => format!("Couldn't extract the file: {e}"),
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
            FileTask::ExtractAll => {
                let mut files = vec![crate::project::ProjectPaths::new(&self.dir).video()];
                files.extend(self.rec.camera.iter().map(|c| self.dir.join(&c.file)));
                files.extend(self.rec.audio.iter().map(|a| self.dir.join(&a.file)));
                let folder = crate::util::unique_path(&target, &format!("{} recording files", self.name));
                std::thread::spawn(move || {
                    let res = std::fs::create_dir_all(&folder).and_then(|_| {
                        for f in files.iter().filter(|f| f.is_file()) {
                            std::fs::copy(f, folder.join(f.file_name().unwrap_or_default()))?;
                        }
                        Ok(())
                    });
                    let msg = match res {
                        Ok(()) => format!("Recording files extracted: {}", crate::util::tilde(&folder)),
                        Err(e) => format!("Couldn't extract the files: {e}"),
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
        }
    }

    /// The Presets button and its dropdown.
    pub(super) fn presets_button(&mut self, ui: &mut egui::Ui) {
        let resp = super::toolbar_button(ui, Icon::Presets, "Presets", "", true, true);
        if resp.clicked() {
            self.presets = presets::list();
            self.preset_menu = None;
        }
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let w = 400.0;
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            let mut apply = None;
            let mut changed = false;
            for (i, p) in self.presets.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
                let gear = Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(28.0, 28.0));
                let gear_resp = ui.interact(gear, egui::Id::NULL.with(("preset-gear", i)), Sense::click());
                let painter = ui.painter();
                if resp.hovered() && !gear_resp.hovered() {
                    painter.rect_filled(r, CornerRadius::same(8), Color32::from_white_alpha(14));
                }
                painter.text(pos2(r.left() + 14.0, r.center().y), egui::Align2::LEFT_CENTER, &p.name, FontId::proportional(14.0), theme::TEXT);
                if self.edit.matches_preset(&p.settings) {
                    let g = painter.layout_no_wrap("Matches project".into(), FontId::proportional(12.5), Color32::from_rgb(0x8e, 0x8f, 0x95));
                    let chip = Rect::from_min_size(pos2(gear.left() - 12.0 - g.size().x - 20.0, r.center().y - 12.0), vec2(g.size().x + 20.0, 24.0));
                    painter.rect_filled(chip, CornerRadius::same(12), Color32::from_white_alpha(12));
                    painter.galley(chip.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
                }
                if gear_resp.hovered() {
                    painter.rect_filled(gear, CornerRadius::same(7), Color32::from_white_alpha(16));
                }
                icons::paint(painter, gear.shrink(5.0), Icon::PresetGear, theme::TEXT);
                if gear_resp.on_hover_text("Preset settings").clicked() {
                    self.preset_menu = if self.preset_menu == Some(i) { None } else { Some(i) };
                } else if resp.on_hover_text("Click to apply").clicked() {
                    apply = Some(i);
                }
                // Preset settings, unfolded under its row.
                if self.preset_menu == Some(i) {
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        if super::menu_item_small(ui, "Update to match project").clicked() {
                            let mut np = p.clone();
                            np.settings = self.edit.clone();
                            if let Err(e) = np.save() {
                                eprintln!("open-studio: {e:#}");
                            }
                            changed = true;
                        }
                        if super::menu_item_small(ui, "Show in folder").clicked() {
                            let _ = std::process::Command::new("xdg-open").arg(presets::dir()).spawn();
                        }
                        if super::menu_item_small(ui, "Delete").clicked() {
                            let _ = p.delete();
                            changed = true;
                        }
                    });
                }
            }
            if let Some(i) = apply {
                let s = self.presets[i].settings.clone();
                self.edit.apply_preset(&s);
            }
            if changed {
                self.presets = presets::list();
                self.preset_menu = None;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Presets are not auto-updated. If you apply preset and then modify project settings - remember to update presets to reflect those changes.")
                            .size(12.5)
                            .color(Color32::from_rgb(0x8a, 0x8b, 0x91)),
                    )
                    .wrap(),
                );
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                let field = ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("New preset name...").desired_width(200.0).margin(vec2(10.0, 8.0)));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let name = self.preset_name.trim().to_string();
                let create = ui.add_enabled(!name.is_empty(), egui::Button::new(egui::RichText::new("Create new preset").color(Color32::WHITE).size(13.5)).fill(theme::ACCENT).min_size(vec2(0.0, 34.0)).corner_radius(CornerRadius::same(8)));
                if (create.clicked() || enter) && !name.is_empty() {
                    match Preset::create(&name, &self.edit) {
                        Ok(_) => {
                            self.preset_name.clear();
                            self.presets = presets::list();
                        }
                        Err(e) => eprintln!("open-studio: creating preset failed: {e:#}"),
                    }
                }
            });
            ui.add_space(6.0);
        });
    }
}
