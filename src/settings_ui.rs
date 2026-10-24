//! The Settings window, laid out like Screen Studio's: General, Recording,
//! Editing, Quick export and Shortcuts tabs. Changes save as they're made.

use crate::editor::widgets;
use crate::icons::{self, Icon};
use crate::settings::{Format, ProRes, Quality, Settings};
use crate::theme;
use egui::{RichText, Sense, Ui, vec2};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    General,
    Recording,
    Editing,
    QuickExport,
    Shortcuts,
}

const TABS: [(Tab, Icon, &str); 5] = [
    (Tab::General, Icon::Gear, "General"),
    (Tab::Recording, Icon::Record, "Recording"),
    (Tab::Editing, Icon::Pencil, "Editing"),
    (Tab::QuickExport, Icon::Export, "Quick export"),
    (Tab::Shortcuts, Icon::Keyboard, "Shortcuts"),
];

pub struct SettingsWindow {
    pub tab: Tab,
    settings: Settings,
    /// Which shortcut is waiting for keys: 0 stop, 1 flag.
    capturing: Option<u8>,
    /// Folder dialog: 0 projects, 1 exports.
    picking: Option<(u8, Receiver<Vec<PathBuf>>)>,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self { tab: Tab::General, settings: Settings::load(), capturing: None, picking: None }
    }
}

/// A setting row: title and help on the left, the control on the right.
fn row(ui: &mut Ui, title: &str, help: &str, control: impl FnOnce(&mut Ui) -> bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 220.0).max(200.0));
            ui.label(RichText::new(title).size(14.0));
            if !help.is_empty() {
                ui.label(RichText::new(help).size(12.5).color(theme::TEXT_DIM));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = control(ui);
        });
    });
    ui.add_space(14.0);
    changed
}

impl SettingsWindow {
    /// Draws the window; false once it's closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        if let Some((which, rx)) = &self.picking {
            match rx.try_recv() {
                Ok(dirs) => {
                    if let Some(d) = dirs.into_iter().next() {
                        if *which == 0 {
                            self.settings.projects_dir = d;
                        } else {
                            self.settings.export_dir = d;
                        }
                        self.settings.save();
                    }
                    self.picking = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        #[allow(deprecated)]
        let id = egui::Id::new("settings-window");
        let modal = egui::Modal::new(id).frame(
            egui::Frame::new()
                .fill(theme::BG)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(16))
                .inner_margin(egui::Margin::same(24)),
        );
        let resp = modal.show(ctx, |ui| {
            ui.set_width(720.0);
            // Tab bar, as in Screen Studio: icon over label.
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - TABS.len() as f32 * 104.0).max(0.0) / 2.0);
                for (tab, icon, name) in TABS {
                    let (r, resp) = ui.allocate_exact_size(vec2(100.0, 58.0), Sense::click());
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if self.tab == tab || resp.hovered() {
                        ui.painter().rect_filled(r, 10.0, if self.tab == tab { theme::SURFACE_HOVER } else { theme::SURFACE });
                    }
                    icons::paint(ui.painter(), egui::Rect::from_center_size(r.center() - vec2(0.0, 10.0), vec2(20.0, 20.0)), icon, theme::TEXT);
                    ui.painter().text(r.center() + vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(12.5), theme::TEXT);
                    if resp.clicked() {
                        self.tab = tab;
                        self.capturing = None;
                    }
                }
            });
            ui.add_space(10.0);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, theme::BORDER);
            ui.add_space(20.0);
            let changed = match self.tab {
                Tab::General => self.general(ui, ctx),
                Tab::Recording => self.recording(ui),
                Tab::Editing => self.editing(ui),
                Tab::QuickExport => self.quick_export(ui),
                Tab::Shortcuts => self.shortcuts(ui),
            };
            if changed {
                self.settings.save();
            }
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    open = false;
                }
            });
        });
        if resp.should_close() && self.capturing.is_none() {
            open = false;
        }
        open
    }

    fn general(&mut self, ui: &mut Ui, ctx: &egui::Context) -> bool {
        for (i, title, dir) in [(0u8, "Save new projects to", self.settings.projects_dir.clone()), (1, "Export videos to", self.settings.export_dir.clone())] {
            row(ui, title, &crate::util::tilde(&dir), |ui| {
                if ui.button("Change directory").clicked() && self.picking.is_none() {
                    self.picking = Some((i, crate::dialogs::open_folder(ctx.clone(), "Choose folder", Some(dir.clone()))));
                }
                false
            });
        }
        false
    }

    fn recording(&mut self, ui: &mut Ui) -> bool {
        let s = &mut self.settings;
        let mut c = false;
        c |= row(ui, "Highlight recorded area during recording", "The part of the screen not being recorded is slightly dimmed.", |ui| widgets::toggle(ui, &mut s.highlight_recorded_area).changed());
        c |= row(ui, "Create zooms automatically", "Your clicks become zooms in every new project.", |ui| widgets::toggle(ui, &mut s.auto_zooms).changed());
        c |= row(ui, "Show quick share widget after recording", "Save or copy the video right away. If off, the editor opens.", |ui| widgets::toggle(ui, &mut s.quick_share).changed());
        c |= row(ui, "Show system cursor in video", "Off records the cursor separately so it can be smoothed and resized.", |ui| widgets::toggle(ui, &mut s.bake_cursor).changed());
        c |= row(ui, "Encode on the GPU", "Far less CPU while recording. Off keeps full colour detail.", |ui| widgets::toggle(ui, &mut s.gpu_encoding).changed());
        c |= row(ui, "Recording countdown", "", |ui| widgets::segmented(ui, &mut s.countdown, &[(0, "Off"), (3, "3s"), (5, "5s"), (10, "10s")]));
        c |= row(ui, "Maximum camera resolution", "Higher resolutions look better but may slow down recording.", |ui| {
            widgets::segmented(ui, &mut s.max_camera_height, &[(720, "720p"), (1080, "1080p"), (2160, "4K")])
        });
        c
    }

    fn editing(&mut self, ui: &mut Ui) -> bool {
        row(
            ui,
            "Use last project settings as default for new recordings",
            "New recordings start with the look of the last project you edited (desktop and phone recordings each keep their own).",
            |ui| widgets::toggle(ui, &mut self.settings.use_last_project_settings).changed(),
        )
    }

    fn quick_export(&mut self, ui: &mut Ui) -> bool {
        let s = &mut self.settings;
        ui.label(RichText::new("These settings are used by the quick share widget, quick export and exporting several projects.").size(12.5).color(theme::TEXT_DIM));
        ui.add_space(12.0);
        let mut c = false;
        c |= row(ui, "Format", "", |ui| widgets::segmented(ui, &mut s.format, &[(Format::Mp4, "MP4"), (Format::Gif, "GIF"), (Format::ProRes, "ProRes")]));
        let fps: &[(u32, &str)] = if s.format == Format::Gif { &[(10, "10"), (15, "15"), (24, "24"), (30, "30")] } else { &[(24, "24"), (25, "25"), (30, "30"), (50, "50"), (60, "60")] };
        if !fps.iter().any(|f| f.0 == s.fps) {
            s.fps = fps.last().unwrap().0;
            c = true;
        }
        c |= row(ui, "Frame rate", "Frames per second.", |ui| widgets::segmented(ui, &mut s.fps, fps));
        let mut h = s.height.unwrap_or(0);
        if row(ui, "Resolution", "Original keeps the recording's size.", |ui| widgets::segmented(ui, &mut h, &[(0, "Original"), (720, "720p"), (1080, "1080p"), (2160, "4K")])) {
            s.height = (h > 0).then_some(h);
            c = true;
        }
        match s.format {
            Format::ProRes => {
                let opts: Vec<(ProRes, &str)> = ProRes::ALL.iter().map(|p| (*p, p.label())).collect();
                c |= row(ui, "ProRes", "Saved as .mov with uncompressed audio.", |ui| widgets::segmented(ui, &mut s.prores, &opts));
            }
            Format::Mp4 => {
                let opts: Vec<(Quality, &str)> = Quality::ALL.iter().map(|q| (*q, q.label())).collect();
                let help = s.quality.description();
                c |= row(ui, "Compression", help, |ui| widgets::segmented(ui, &mut s.quality, &opts));
            }
            Format::Gif => {}
        }
        c
    }

    fn shortcuts(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        for (i, title, help) in [
            (0u8, "Start new recording / Finish recording", "Stops the recording in progress from anywhere."),
            (1, "Create recording flag", "Marks a moment while recording; it shows as a flag on the timeline."),
        ] {
            let value = if i == 0 { self.settings.stop_shortcut.clone() } else { self.settings.flag_shortcut.clone() };
            let capturing = self.capturing == Some(i);
            row(ui, title, help, |ui| {
                let label = if capturing { "Press keys…".to_string() } else { value };
                if ui.add(egui::Button::new(RichText::new(label).size(13.0)).min_size(vec2(150.0, 32.0))).on_hover_text("Click to change").clicked() {
                    self.capturing = if capturing { None } else { Some(i) };
                }
                false
            });
        }
        if let Some(i) = self.capturing {
            if let Some(combo) = captured(ui) {
                if crate::record::input::Shortcut::parse(&combo).is_some() {
                    if i == 0 {
                        self.settings.stop_shortcut = combo;
                    } else {
                        self.settings.flag_shortcut = combo;
                    }
                    c = true;
                }
                self.capturing = None;
            }
        }
        ui.label(RichText::new("Speaker notes: Ctrl+Alt+/ shows them, Ctrl+Alt+. starts and stops the prompter.").size(12.5).color(theme::TEXT_DIM));
        c
    }
}

/// A key combination pressed this frame ("Ctrl+Alt+F"), Escape excluded.
fn captured(ui: &Ui) -> Option<String> {
    ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, .. } if *key != egui::Key::Escape => {
                let mut parts = vec![];
                if modifiers.ctrl {
                    parts.push("Ctrl".to_string());
                }
                if modifiers.alt {
                    parts.push("Alt".to_string());
                }
                if modifiers.shift {
                    parts.push("Shift".to_string());
                }
                if modifiers.mac_cmd || (modifiers.command && !modifiers.ctrl) {
                    parts.push("Super".to_string());
                }
                parts.push(key.name().to_string());
                Some(parts.join("+"))
            }
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tabs switch, a toggle saves at once, and a shortcut is captured.
    #[test]
    fn settings_window_saves_changes() {
        let dir = std::env::temp_dir().join("open-studio-settings-ui-test");
        let _ = std::fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let ctx = egui::Context::default();
        let mut w = SettingsWindow::default();
        let mut t = 0.0;
        let mut frame = |w: &mut SettingsWindow, events: Vec<egui::Event>| {
            t += 0.05;
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 900.0))), time: Some(t), events, ..Default::default() };
            let mut open = true;
            let mut out = ctx.run_ui(raw, |ui| open = w.show(ui.ctx()));
            out.textures_delta.clear();
            open
        };
        assert!(frame(&mut w, vec![]));
        for tab in [Tab::Recording, Tab::Editing, Tab::QuickExport, Tab::Shortcuts, Tab::General] {
            w.tab = tab;
            assert!(frame(&mut w, vec![]), "{tab:?} closed the window");
        }
        // Shortcut capture: start it, press keys.
        w.tab = Tab::Shortcuts;
        w.capturing = Some(1);
        let mods = egui::Modifiers { ctrl: true, shift: true, command: true, ..Default::default() };
        frame(&mut w, vec![egui::Event::Key { key: egui::Key::M, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        assert_eq!(w.capturing, None);
        assert_eq!(Settings::load().flag_shortcut, "Ctrl+Shift+M");
        assert_eq!(Settings::load().stop_shortcut, Settings::default().stop_shortcut);
    }
}
