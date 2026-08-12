//! UI layer. Widgets are built in pure Rust (no `.ui` templates) — the app is
//! small enough that builder-pattern construction stays legible.
//!
//! [`window`] builds the whole calculator (display + keypads + history) and is
//! entered from [`crate::app::build_ui`]. This module owns the stylesheet.

pub mod window;

use gtk::gdk;

/// The app stylesheet. libadwaita *named* colors (`@accent_color`,
/// `@window_fg_color`, `@error_color`, …) are used throughout
/// so every surface follows the system light/dark theme AND the user's accent
/// color automatically, so the calculator is not tied to a fixed
/// purple/pink palette.
pub const APP_CSS: &str = "
/* ─── Display ──────────────────────────────────────────────────────────── */

/* The current expression, right-aligned, large. */
.calc-expression {
    font-size: 2.6em;
    font-weight: 300;
    color: @window_fg_color;
}

/* The live result / (in Result state) the emphasized answer. Accent colored. */
.calc-result {
    font-size: 1.5em;
    font-weight: 400;
    color: @accent_color;
}

/* In Result state the two labels swap emphasis: the expression dims and
   shrinks while the result grows into the primary line. */
.calc-expression.calc-secondary {
    font-size: 1.5em;
    opacity: 0.6;
}
.calc-result.calc-primary {
    font-size: 2.8em;
    font-weight: 400;
    color: @window_fg_color;
}

/* Error state recolors both display lines. */
.calc-error { color: @error_color; }

/* Small persistent DEG/RAD + M / memory indicators under the header. */
.calc-indicator {
    font-size: 0.8em;
    font-weight: bold;
    opacity: 0.7;
    color: @accent_color;
}

/* ─── Keypad — round basic keys ───────────────────────────────────────── */

.calc-btn {
    font-size: 1.5em;
    font-weight: 500;
    min-width: 62px;
    min-height: 62px;
    border-radius: 9999px;   /* circular */
    padding: 0;
    transition: background-color 100ms ease, filter 100ms ease;
}

/* Pressed-key feedback. NOTE: :active is the
   PRESSED pseudo-class — distinct from the .calc-active toggle CLASS. */
.calc-btn:active {
    filter: brightness(0.90);
}

/* Programmer keypad keys: shorter than the 62px basic keys so all 7 rows
   fit a 432×916 viewport without clipping the bottom (=) row. */
.calc-btn-prog {
    min-height: 52px;
}

/* Digits & the decimal point: subtle raised neutral. */
.calc-digit {
    background-color: alpha(@window_fg_color, 0.06);
    color: @window_fg_color;
}

/* Operators ÷ × − + : accent-tinted. */
.calc-operator {
    background-color: alpha(@accent_bg_color, 0.15);
    color: @accent_color;
    font-weight: 600;
}

/* AC / clear: a stronger accent highlight. */
.calc-clear {
    background-color: alpha(@accent_bg_color, 0.28);
    color: @accent_color;
    font-weight: 600;
}

/* ⌫ and the scientific functions: neutral. */
.calc-function {
    background-color: alpha(@window_fg_color, 0.08);
    color: @window_fg_color;
}

/* = : the filled primary action. */
.calc-equals {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
    font-weight: 600;
}

/* An active toggle (Inv on, or the current Deg/Rad mode). */
.calc-active {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}

/* ─── Scientific pad — pill/stadium keys (multi-char labels) ───────────── */
/* min-height reduced to 38px so the expanded 6-row scientific pad fits a
   432×916 portrait viewport above the 5-row basic pad without clipping the
   bottom (0 . ⌫ =) row (degrades acceptably on 380×780). GTK adds internal
   padding, so the visual tap target stays comfortably >~44px. */

.calc-sci {
    font-size: 1.1em;
    min-width: 56px;
    min-height: 38px;
    border-radius: 9999px;   /* stadium */
    padding: 0 6px;
    transition: background-color 100ms ease, filter 100ms ease;
}

/* Pressed-key feedback for the pill scientific keys. */
.calc-sci:active {
    filter: brightness(0.90);
}

/* The chevron handle that reveals the scientific pad. */
.calc-chevron {
    min-height: 22px;
    padding: 0;
    background: none;
    box-shadow: none;
    color: @window_fg_color;
    opacity: 0.55;
}
.calc-chevron:hover { opacity: 0.9; }

/* ─── History rows ────────────────────────────────────────────────────── */

.calc-hist-expr {
    font-size: 1.0em;
    opacity: 0.6;
}
.calc-hist-result {
    font-size: 1.5em;
    color: @window_fg_color;
}

/* ─── Landscape overrides ─────────────────────────────────────────────── */

/* Landscape display: a compact top strip, not the tall portrait block. */
.calc-display.landscape {
    margin-top: 2px;
    margin-bottom: 2px;
}
/* Landscape: compact display fonts so the indicator + expression + result
   fit in the fixed 110px display strip and the Input->Result emphasis swap
   never exceeds it (keeps the vexpanding keypad's height constant). */
.calc-display.landscape .calc-expression { font-size: 1.6em; }
.calc-display.landscape .calc-expression.calc-secondary { font-size: 1.1em; }
.calc-display.landscape .calc-result { font-size: 1.1em; }
.calc-display.landscape .calc-result.calc-primary { font-size: 1.6em; }
.calc-display.landscape .calc-indicator { font-size: 0.75em; }

/* Landscape keypad buttons shrink so all 5 rows fit the short height. The
   portrait keypad keeps its fixed 62/46px min-heights (separate instances). */
.calc-btn-land {
    min-height: 0;
    min-width: 0;
    font-size: 1.25em;
}
.calc-sci-land {
    min-height: 0;
    min-width: 0;
    font-size: 1.0em;
}

/* ─── Programmer mode ─────────────────────────────────────────────────── */

/* Base rows: flat, with the active base row getting an accent-tinted bg. */
.calc-prog-row {
    padding: 4px 8px;
    border-radius: 8px;
    background: none;
}
.calc-prog-row.calc-primary {
    background-color: alpha(@accent_bg_color, 0.18);
}
.calc-prog-baselabel {
    font-size: 0.85em;
    font-weight: bold;
    opacity: 0.7;
    color: @accent_color;
}
.calc-prog-value {
    font-size: 1.5em;
    font-weight: 400;
    font-family: monospace;
    color: @window_fg_color;
}
.calc-prog-value.calc-error {
    color: @error_color;
}
.calc-disabled {
    opacity: 0.35;
}

/* ─── Financial mode ──────────────────────────────────────────────────── */

/* Field rows: flat, with the active field row getting an accent-tinted bg. */
.calc-fin-row {
    padding: 4px 8px;
    border-radius: 8px;
    background: none;
}
.calc-fin-row.calc-primary {
    background-color: alpha(@accent_bg_color, 0.18);
}
.calc-fin-label {
    opacity: 0.8;
    color: @window_fg_color;
}
.calc-fin-value {
    font-family: monospace;
    color: @window_fg_color;
}
.calc-fin-suffix {
    font-size: 0.8em;
    opacity: 0.5;
    color: @window_fg_color;
}
.calc-fin-result {
    font-size: 1.4em;
    font-weight: 500;
    color: @accent_color;
    font-family: monospace;
}
.calc-fin-result.calc-error {
    color: @error_color;
}
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
