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
use crate::engine::format::NumLocale;
use crate::programmer::{Base, Width};

/// GSettings keys — kept in one place so they stay in sync with the gschema.
const KEY_WIDTH: &str = "window-width";
const KEY_HEIGHT: &str = "window-height";
const KEY_MAXIMIZED: &str = "window-maximized";
const KEY_ANGLE: &str = "angle-mode";
const KEY_INVERSE: &str = "inverse-mode";
const KEY_CONVERTER_CATEGORY: &str = "converter-category";
const KEY_NUMBER_FORMAT: &str = "number-format";
const KEY_ACTIVE_MODE: &str = "active-mode";
const KEY_PROG_BASE: &str = "prog-base";
const KEY_PROG_BIT_WIDTH: &str = "prog-bit-width";
const KEY_PROG_SIGNED: &str = "prog-signed";
const KEY_LAST_CALC_MODE: &str = "last-calc-mode";

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

/// The saved number-format locale, defaulting to en-US.
pub fn number_format() -> NumLocale {
    let raw = settings()
        .map(|s| s.string(KEY_NUMBER_FORMAT).to_string())
        .unwrap_or_else(|| "en-us".to_string());
    if raw == "es-ar" {
        NumLocale::EsAr
    } else {
        NumLocale::EnUs
    }
}

/// Persist the number-format locale (best-effort).
pub fn set_number_format(locale: NumLocale) {
    if let Some(s) = settings() {
        let value = match locale {
            NumLocale::EnUs => "en-us",
            NumLocale::EsAr => "es-ar",
        };
        let _ = s.set_string(KEY_NUMBER_FORMAT, value);
    }
}

/// The saved active mode, defaulting to "calculator".
pub fn active_mode() -> String {
    settings()
        .map(|s| s.string(KEY_ACTIVE_MODE).to_string())
        .unwrap_or_else(|| "calculator".to_string())
}

/// Persist the active mode (best-effort).
pub fn set_active_mode(mode: &str) {
    if let Some(s) = settings() {
        let _ = s.set_string(KEY_ACTIVE_MODE, mode);
    }
}

/// The saved calculator-family mode to return to when leaving the
/// converter, defaulting to "calculator".
pub fn last_calc_mode() -> String {
    settings()
        .map(|s| s.string(KEY_LAST_CALC_MODE).to_string())
        .unwrap_or_else(|| "calculator".to_string())
}

/// Persist the calculator-family return mode (best-effort).
pub fn set_last_calc_mode(mode: &str) {
    if let Some(s) = settings() {
        let _ = s.set_string(KEY_LAST_CALC_MODE, mode);
    }
}

/// The saved programmer-mode base, defaulting to decimal.
pub fn prog_base() -> Base {
    let raw = settings()
        .map(|s| s.string(KEY_PROG_BASE).to_string())
        .unwrap_or_else(|| "dec".to_string());
    match raw.as_str() {
        "hex" => Base::Hex,
        "oct" => Base::Oct,
        "bin" => Base::Bin,
        _ => Base::Dec,
    }
}

/// Persist the programmer-mode base (best-effort).
pub fn set_prog_base(base: Base) {
    if let Some(s) = settings() {
        let value = match base {
            Base::Hex => "hex",
            Base::Dec => "dec",
            Base::Oct => "oct",
            Base::Bin => "bin",
        };
        let _ = s.set_string(KEY_PROG_BASE, value);
    }
}

/// The saved programmer-mode bit width, defaulting to 32.
pub fn prog_width() -> Width {
    let raw = settings().map(|s| s.int(KEY_PROG_BIT_WIDTH)).unwrap_or(32);
    match raw {
        8 => Width::W8,
        16 => Width::W16,
        64 => Width::W64,
        _ => Width::W32,
    }
}

/// Persist the programmer-mode bit width (best-effort).
pub fn set_prog_width(width: Width) {
    if let Some(s) = settings() {
        let value = match width {
            Width::W8 => 8,
            Width::W16 => 16,
            Width::W32 => 32,
            Width::W64 => 64,
        };
        let _ = s.set_int(KEY_PROG_BIT_WIDTH, value);
    }
}

/// The saved programmer-mode signedness, defaulting to signed (true).
pub fn prog_signed() -> bool {
    settings().map(|s| s.boolean(KEY_PROG_SIGNED)).unwrap_or(true)
}

/// Persist the programmer-mode signedness (best-effort).
pub fn set_prog_signed(v: bool) {
    if let Some(s) = settings() {
        let _ = s.set_boolean(KEY_PROG_SIGNED, v);
    }
}
