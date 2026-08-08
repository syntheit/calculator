# Calculator design

Calculator is a native GTK4/libadwaita calculator in the style of Google
Calculator. It is built the same way as Warden (Bitwarden), Courier (email) and
Jotter (Memos): Rust, gtk4-rs 0.11, libadwaita 0.9, an `AdwApplicationWindow`
shell, and a Nix flake with crane + fenix. It targets a Linux phone (GNOME Shell
Mobile, aarch64) so every screen is mobile-adaptive, and it scales up to the
desktop.

App id: `io.matv.Calculator`.

## Principles

- **Pure-Rust builders, no `.ui` templates.** The app is small enough that
  builder-pattern widget construction stays legible (same approach as Warden and
  Courier).
- **Fully synchronous.** A calculator does no I/O beyond loading/saving the
  history file, so there is no tokio and no async channel — everything runs on
  the GTK main thread.
- **No on-screen-keyboard-triggering widgets.** The display is a non-editable
  `GtkLabel`, never an editable entry, so tapping it does not raise the OSK on
  the phone. Input comes from the on-screen keypad buttons and from a hardware
  keyboard via an `EventControllerKey` attached to the window.
- **libadwaita named-color theming.** The stylesheet uses `@accent_color`,
  `@card_bg_color`, etc., so the app follows the system light/dark theme. Custom
  named colors are used for the keypad accents.

## Architecture (to be implemented)

The following modules are planned; only the shell (`main.rs`, `app.rs`,
`ui/mod.rs`) exists so far.

- `src/engine/` — a hand-rolled recursive-descent expression evaluator over
  `f64`. Tokeniser + Pratt/recursive-descent parser, honoring operator
  precedence and parentheses, the scientific function set (sin/cos/tan and
  inverses, log/ln, exp, sqrt, powers, factorial, constants π and e) and the
  active angle mode (deg/rad). Pure and unit-tested; no GTK dependency. *To be
  implemented.*
- `src/state.rs` — the calculator state machine: the current expression buffer,
  cursor/entry rules, the live-result computation, the memory register
  (MS/MR/M+/M−) and how keypad/keyboard events mutate the buffer. Pure and
  unit-tested. *To be implemented.*
- `src/history.rs` — the calculation history, persisted to the XDG data dir
  (`$XDG_DATA_HOME/calculator/history.json`) as JSON via serde. *To be
  implemented.*
- `src/ui/` — the widgets: the non-editable display (expression + live result),
  the basic keypad, the collapsible scientific pad, the deg/rad toggle, the
  history list and the memory controls. Assembled inside the
  `AdwApplicationWindow` shell in `src/app.rs`. *To be implemented.*

## Settings

Persisted via `GSettings` (`io.matv.Calculator`): window geometry
(`window-width` / `window-height` / `window-maximized`) and the angle mode
(`angle-mode`, `"rad"` or `"deg"`).

## Layout

An `AdwApplicationWindow` with an `AdwToolbarView` (an `AdwHeaderBar` on top, the
calculator body below). An `AdwBreakpoint` at 550 sp marks the phone/desktop
cutover; below it the layout is single-column phone-first, above it the
scientific pad and history can sit alongside the keypad.
