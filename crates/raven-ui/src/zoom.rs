//! The icon size as one value shared by every control that changes it: the
//! status bar's zoom slider, Ctrl+scroll over the file views, and the
//! Settings dialog's "Icon size" row. All of them move the same
//! [`gtk::Adjustment`], so they never disagree.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::glib;
use gtk4 as gtk;
use gtk::prelude::*;

use crate::state::AppState;
use crate::themes;

pub const MIN_ICON_SIZE: f64 = 16.0;
pub const MAX_ICON_SIZE: f64 = 64.0;
/// One zoom step: a scroll notch, a click on a zoom button.
pub const STEP: f64 = 4.0;

/// How long the size has to stay put before the config is written: dragging
/// the slider moves it many times a second.
const SAVE_SETTLE: Duration = Duration::from_millis(400);

thread_local! {
    static ADJUSTMENT: RefCell<Option<gtk::Adjustment>> = const { RefCell::new(None) };
}

/// The shared icon-size adjustment, created from the config on first use.
pub fn icon_size_adjustment(state: &AppState) -> gtk::Adjustment {
    ADJUSTMENT.with(|cell| {
        if let Some(adjustment) = cell.borrow().as_ref() {
            return adjustment.clone();
        }
        let size = f64::from(state.borrow().config.appearance.icon_size)
            .clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        let adjustment = gtk::Adjustment::new(size, MIN_ICON_SIZE, MAX_ICON_SIZE, 2.0, STEP, 0.0);

        let state = state.clone();
        let pending_save: std::rc::Rc<Cell<Option<glib::SourceId>>> = Default::default();
        adjustment.connect_value_changed(move |adjustment| {
            let size = adjustment.value().round() as u32;
            let font_size = {
                let mut s = state.borrow_mut();
                if s.config.appearance.icon_size == size {
                    return;
                }
                s.config.appearance.icon_size = size;
                s.config.appearance.font_size
            };
            themes::apply_sizes(font_size, size);

            if let Some(id) = pending_save.take() {
                id.remove();
            }
            let state = state.clone();
            let pending = pending_save.clone();
            pending_save.set(Some(glib::timeout_add_local_once(SAVE_SETTLE, move || {
                pending.set(None);
                let _ = state.borrow().config.save();
            })));
        });

        *cell.borrow_mut() = Some(adjustment.clone());
        adjustment
    })
}

/// Zoom in (`steps > 0`) or out by whole steps.
pub fn zoom_by(state: &AppState, steps: f64) {
    let adjustment = icon_size_adjustment(state);
    adjustment.set_value(adjustment.value() + steps * STEP);
}
