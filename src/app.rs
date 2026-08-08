//! Application shell. The real window — display, scientific/basic keypad,
//! history and memory — is built by [`crate::ui::window`]; this module is the
//! thin activation entry point the [`adw::Application`] calls into. See
//! DESIGN.md.

/// Build the main window and present it.
pub fn build_ui(app: &adw::Application) {
    crate::ui::window::build_ui(app);
}
