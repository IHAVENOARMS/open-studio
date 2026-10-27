//! What the desktop we run on can do, probed once at startup. Features check
//! these capabilities instead of asking which compositor this is, so each one
//! picks its best mechanism and falls back where it is missing.

pub mod hyprland;

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

    /// The floating recording bar with direct capture. Elsewhere the
    /// projects window and the ScreenCast portal are used.
    pub fn recording_bar(&self) -> bool {
        self.hyprland_ipc
    }

    /// The pointer position can be polled in global coordinates.
    pub fn pointer_poll(&self) -> bool {
        self.hyprland_ipc
    }

    /// Window positions are known, so a recorded window can be followed.
    pub fn window_geometry(&self) -> bool {
        self.hyprland_ipc
    }

    /// Our own window can be put away while recording and brought back.
    pub fn hide_own_window(&self) -> bool {
        self.hyprland_ipc
    }
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
        assert!(c.image_copy_capture && c.toplevel_list && !c.toplevel_capture && !c.recording_bar());

        // GNOME: none of these.
        let c = Caps::from_globals(["wl_compositor", "xdg_wm_base"], false);
        assert!(c.wayland && !c.layer_shell && !c.image_copy_capture && !c.pointer_poll());
    }
}
