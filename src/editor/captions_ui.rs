//! Captions and Transcript panels.

use crate::edit::EditConfig;
use crate::project::Recording;
use crate::theme;
use egui::{RichText, Ui};

#[derive(Default)]
pub struct State {}

pub fn captions(ui: &mut Ui, _st: &mut State, _edit: &mut EditConfig, _rec: &Recording) {
    ui.label(RichText::new("Captions").font(theme::semibold(15.0)));
}

pub fn transcript(ui: &mut Ui, _st: &mut State, _edit: &mut EditConfig, _rec: &Recording) -> Option<f64> {
    ui.label(RichText::new("Transcript").font(theme::semibold(15.0)));
    None
}
