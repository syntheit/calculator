//! Non-secret UI preferences backed by GSettings (schema id [`crate::APP_ID`]).
//!
//! Wraps [`gio::Settings`] so the UI never touches raw key strings. The devShell
//! points `GSETTINGS_SCHEMA_DIR` at the locally compiled schema; the installed
//! app ships it under `share/glib-2.0/schemas`. Construction is guarded so a
//! missing schema (e.g. a bare `cargo run` outside the devShell) degrades to
//! in-memory defaults rather than aborting the process.

use gtk::gio;
use gtk::prelude::*;

use crate::engine::AngleUnit;

/// GSettings keys — kept in one place so they stay in sync with the gschema.
const KEY_WIDTH: &str = "window-width";
const KEY_HEIGHT: &str = "window-height";
const KEY_MAXIMIZED: &str = "window-maximized";
const KEY_ANGLE: &str = "angle-mode";
const KEY_INVERSE: &str = "inverse-mode";
const KEY_CONVERTER_CATEGORY: &str = "converter-category";

/// Open the settings store, or `None` when the schema isn't installed.
///
/// `gio::Settings::new` aborts the process on an unknown schema, so we first
/// look the schema up in the default source and only construct settings when it
/// is actually present.
fn settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    // `recursive = true` so schemas from GSETTINGS_SCHEMA_DIR are found too.
    source.lookup(crate::APP_ID, true)?;
    Some(gio::Settings::new(crate::APP_ID))
}

/// The last saved window size as `(width, height)`, falling back to the
/// phone-first default when no schema is available.
pub fn window_size() -> (i32, i32) {
    match settings() {
        Some(s) => (s.int(KEY_WIDTH), s.int(KEY_HEIGHT)),
        None => (380, 780),
    }
}

/// Persist the window size (best-effort).
pub fn set_window_size(w: i32, h: i32) {
    if let Some(s) = settings() {
        let _ = s.set_int(KEY_WIDTH, w);
        let _ = s.set_int(KEY_HEIGHT, h);
    }
}

/// Whether the window was maximized when last closed.
pub fn window_maximized() -> bool {
    settings().map(|s| s.boolean(KEY_MAXIMIZED)).unwrap_or(false)
}

/// Persist the window maximized state (best-effort).
pub fn set_window_maximized(maximized: bool) {
    if let Some(s) = settings() {
        let _ = s.set_boolean(KEY_MAXIMIZED, maximized);
    }
}

/// The saved trig angle mode, defaulting to radians.
pub fn angle_mode() -> AngleUnit {
    let raw = settings()
        .map(|s| s.string(KEY_ANGLE).to_string())
        .unwrap_or_else(|| "rad".to_string());
    if raw == "deg" {
        AngleUnit::Deg
    } else {
        AngleUnit::Rad
    }
}

/// Persist the trig angle mode (best-effort).
pub fn set_angle_mode(angle: AngleUnit) {
    if let Some(s) = settings() {
        let value = match angle {
            AngleUnit::Deg => "deg",
            AngleUnit::Rad => "rad",
        };
        let _ = s.set_string(KEY_ANGLE, value);
    }
}

/// The saved scientific inverse-mode flag, defaulting to off.
pub fn inverse_mode() -> bool {
    settings().map(|s| s.boolean(KEY_INVERSE)).unwrap_or(false)
}

/// Persist the scientific inverse-mode flag (best-effort).
pub fn set_inverse_mode(v: bool) {
    if let Some(s) = settings() {
        let _ = s.set_boolean(KEY_INVERSE, v);
    }
}

/// The saved unit-converter category, defaulting to Length.
pub fn converter_category() -> crate::convert::Category {
    use crate::convert::Category;
    let raw = settings()
        .map(|s| s.string(KEY_CONVERTER_CATEGORY).to_string())
        .unwrap_or_else(|| "length".to_string());
    Category::all()
        .iter()
        .copied()
        .find(|c| c.name().to_lowercase() == raw)
        .unwrap_or(Category::Length)
}

/// Persist the unit-converter category (best-effort).
pub fn set_converter_category(cat: crate::convert::Category) {
    if let Some(s) = settings() {
        let _ = s.set_string(KEY_CONVERTER_CATEGORY, &cat.name().to_lowercase());
    }
}
