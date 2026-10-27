//! Standard Wayland queries that work on any compositor: the monitor
//! layout from wl_output and xdg_output.

use super::Monitor;
use anyhow::Result;
use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, QueueHandle, globals::registry_queue_init};

struct State {
    registry: RegistryState,
    outputs: OutputState,
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_registry!(State);
smithay_client_toolkit::delegate_dispatch2!(State);

pub fn monitors() -> Result<Vec<Monitor>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let mut state = State { registry: RegistryState::new(&globals), outputs: OutputState::new(&globals, &qh) };
    // Outputs, then their xdg_output details.
    for _ in 0..3 {
        queue.roundtrip(&mut state)?;
    }
    let mut list: Vec<Monitor> = state.outputs.outputs().filter_map(|o| state.outputs.info(&o)).filter_map(|i| from_info(&i)).collect();
    list.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]).then(a.rect[1].total_cmp(&b.rect[1])));
    Ok(list)
}

fn from_info(i: &OutputInfo) -> Option<Monitor> {
    let mode = i.modes.iter().find(|m| m.current).or(i.modes.first())?;
    let (mw, mh) = (mode.dimensions.0 as u32, mode.dimensions.1 as u32);
    let rotated = matches!(
        i.transform,
        wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270
    );
    let pixels = if rotated { (mh, mw) } else { (mw, mh) };
    // xdg_output gives the logical layout, fractional scales included;
    // without it, wl_output's integer scale.
    let scale = i.scale_factor.max(1) as f64;
    let (x, y) = i.logical_position.unwrap_or(i.location);
    let (w, h) = match i.logical_size {
        Some((w, h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (pixels.0 as f64 / scale, pixels.1 as f64 / scale),
    };
    let name = i.name.clone().unwrap_or_else(|| format!("output-{}", i.id));
    let make_model = format!("{} {}", i.make, i.model).trim().to_string();
    let description = if make_model.is_empty() || make_model == "Unknown Unknown" { name.clone() } else { make_model };
    Some(Monitor {
        name,
        description,
        rect: [x as f64, y as f64, w, h],
        pixels,
        refresh: if mode.refresh_rate > 0 { mode.refresh_rate as f64 / 1000.0 } else { 60.0 },
    })
}
