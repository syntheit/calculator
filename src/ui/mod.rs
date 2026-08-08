//! UI layer. Widgets are built in pure Rust (no `.ui` templates) — the app is
//! small enough that builder-pattern construction stays legible.
//!
//! This module currently holds only the stylesheet loader. The keypad, display,
//! history and memory widgets are added by later work.

use gtk::gdk;

/// The app stylesheet. Placeholder set of calculator-oriented classes; the UI
/// work will expand this (key styling, the live-result display, the history
/// list, etc.). libadwaita named colors (`@accent_color`, `@card_bg_color`, …)
/// are used so the sheet follows the system light/dark theme.
pub const APP_CSS: &str = "
/* --- Calculator stylesheet (placeholder — expanded by the UI work) --- */

/* Keypad keys */
.calc-key { font-size: 1.4em; border-radius: 12px; }
.calc-key.operator { color: @accent_color; }
.calc-key.function { font-size: 1.1em; }

/* The display: the current expression and the live result. Non-editable. */
.calc-expression { font-size: 2.0em; }
.calc-result { font-size: 1.3em; opacity: 0.7; }
";

/// Install the app stylesheet. Call once at application startup.
pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(APP_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
