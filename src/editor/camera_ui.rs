//! Camera Layout and Camera Style panels.

use crate::edit::EditConfig;
use crate::project::Recording;
use crate::theme;
use egui::{RichText, Ui};

pub fn show(ui: &mut Ui, _edit: &mut EditConfig, _rec: &Recording, layout: bool) {
    ui.label(RichText::new(if layout { "Camera Layout" } else { "Camera Style" }).font(theme::semibold(15.0)));
    ui.label(RichText::new("Record with a camera to use this.").size(12.0).color(theme::TEXT_DIM));
}
