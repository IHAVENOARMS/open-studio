//! What the desktop we run on can do, probed once at startup. Features check
//! these capabilities instead of asking which compositor this is, so each one
//! picks its best mechanism and falls back where it is missing.

pub mod hyprland;
mod wayland;

use std::sync::OnceLock;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalListContents, globals::registry_queue_init};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caps {
    /// A Wayland session (as opposed to X11).
    pub wayland: bool,
    /// `zwlr_layer_shell_v1`: panels and overlays (wlroots, KDE; not GNOME).
    pub layer_shell: bool,
    /// `ext_image_copy_capture_manager_v1` plus output sources: direct
    /// monitor capture without the portal.
    pub image_copy_capture: bool,
    /// Direct capture of single windows.
    pub toplevel_capture: bool,
    /// `ext_foreign_toplevel_list_v1`: titles and app ids of open windows.
    pub toplevel_list: bool,
    /// `zwlr_screencopy_manager_v1`.
    pub wlr_screencopy: bool,
    /// Hyprland's IPC socket: pointer position, window geometry, rules.
    pub hyprland_ipc: bool,
}

impl Caps {
    fn from_globals<'a>(globals: impl IntoIterator<Item = &'a str>, hyprland_ipc: bool) -> Self {
        let names: Vec<&str> = globals.into_iter().collect();
        let has = |n: &str| names.contains(&n);
        Caps {
            wayland: true,
            layer_shell: has("zwlr_layer_shell_v1"),
            image_copy_capture: has("ext_image_copy_capture_manager_v1") && has("ext_output_image_capture_source_manager_v1"),
            toplevel_capture: has("ext_image_copy_capture_manager_v1")
                && has("ext_foreign_toplevel_image_capture_source_manager_v1")
                && has("ext_foreign_toplevel_list_v1"),
            toplevel_list: has("ext_foreign_toplevel_list_v1"),
            wlr_screencopy: has("zwlr_screencopy_manager_v1"),
            hyprland_ipc,
        }
    }

    fn probe() -> Self {
        // OPEN_STUDIO_DESKTOP=generic ignores compositor-specific IPC, to
        // try the portable paths on a Hyprland machine.
        let generic = std::env::var("OPEN_STUDIO_DESKTOP").is_ok_and(|v| v == "generic");
        let hyprland_ipc = !generic && hyprland::available();
        match wayland_globals() {
            Some(g) => Self::from_globals(g.iter().map(String::as_str), hyprland_ipc),
            None => Caps { hyprland_ipc, ..Default::default() },
        }
    }

    /// The floating recording bar with direct capture: needs layer-shell
    /// overlays and output capture (wlroots, KDE, Hyprland). Elsewhere the
    /// projects window and the ScreenCast portal are used.
    pub fn recording_bar(&self) -> bool {
        self.layer_shell && self.image_copy_capture
    }

    /// The pointer position can be polled in global coordinates.
    pub fn pointer_poll(&self) -> bool {
        self.hyprland_ipc
    }

    /// Window positions are known, so a recorded window can be followed.
    pub fn window_geometry(&self) -> bool {
        self.hyprland_ipc
    }
}

/// A monitor in the global layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    /// Make and model, for showing to people.
    pub description: String,
    /// Logical x, y, width, height in the global layout.
    pub rect: [f64; 4],
    /// Size in pixels as shown (after rotation).
    pub pixels: (u32, u32),
    pub refresh: f64,
}

impl Monitor {
    /// Pixels per logical unit.
    pub fn scale(&self) -> f64 {
        self.pixels.0 as f64 / self.rect[2].max(1.0)
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let r = self.rect;
        x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
    }
}

/// The monitor layout, from Hyprland IPC there and from wl_output and
/// xdg_output elsewhere.
pub fn monitors() -> anyhow::Result<Vec<Monitor>> {
    if !caps().hyprland_ipc {
        return wayland::monitors();
    }
    let infos = hyprland::monitor_infos().unwrap_or_default();
    Ok(hyprland::monitors()?
        .into_iter()
        .map(|m| {
            let info = infos.iter().find(|i| i.name == m.name);
            let (pw, ph) = m.pixel_size();
            let desc = info.map(|i| i.description.clone()).unwrap_or_default();
            // Hyprland descriptions end with serial and connector; keep make and model.
            let short = desc.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
            Monitor {
                name: m.name.clone(),
                description: if short.is_empty() { m.name.clone() } else { short },
                rect: m.logical_rect(),
                pixels: (pw as u32, ph as u32),
                refresh: info.map(|i| i.refresh_rate).unwrap_or(60.0),
            }
        })
        .collect())
}

/// Monitors from the standard protocols even on Hyprland, to compare.
pub fn wayland_monitors() -> anyhow::Result<Vec<Monitor>> {
    wayland::monitors()
}

/// Where the pointer is in the global layout, where the desktop says.
pub fn pointer_pos() -> Option<(f64, f64)> {
    caps().pointer_poll().then(|| hyprland::cursor_pos().ok()).flatten()
}

/// The capabilities of this session, probed on first use.
pub fn caps() -> &'static Caps {
    static CAPS: OnceLock<Caps> = OnceLock::new();
    CAPS.get_or_init(Caps::probe)
}

struct Probe;

impl Dispatch<WlRegistry, GlobalListContents> for Probe {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

/// Interface names the compositor advertises, or None outside Wayland.
fn wayland_globals() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, _queue) = registry_queue_init::<Probe>(&conn).ok()?;
    Some(globals.contents().clone_list().into_iter().map(|g| g.interface).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_from_globals() {
        let hypr = ["wl_compositor", "zwlr_layer_shell_v1", "ext_image_copy_capture_manager_v1", "ext_output_image_capture_source_manager_v1", "ext_foreign_toplevel_list_v1", "ext_foreign_toplevel_image_capture_source_manager_v1", "zwlr_screencopy_manager_v1"];
        let c = Caps::from_globals(hypr, true);
        assert!(c.layer_shell && c.image_copy_capture && c.toplevel_capture && c.wlr_screencopy && c.recording_bar());

        // Sway: output capture and a window list, but no window capture.
        let sway = ["zwlr_layer_shell_v1", "ext_image_copy_capture_manager_v1", "ext_output_image_capture_source_manager_v1", "ext_foreign_toplevel_list_v1"];
        let c = Caps::from_globals(sway, false);
        assert!(c.image_copy_capture && c.toplevel_list && !c.toplevel_capture && c.recording_bar() && !c.window_geometry());

        // GNOME: none of these.
        let c = Caps::from_globals(["wl_compositor", "xdg_wm_base"], false);
        assert!(c.wayland && !c.layer_shell && !c.image_copy_capture && !c.pointer_poll() && !c.recording_bar());
    }
}
