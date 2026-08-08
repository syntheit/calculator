//! Application shell: the top-level window. This is a placeholder that proves
//! the GTK/libadwaita stack builds and opens a window. The real content — the
//! display, scientific/basic keypad, history and memory — is built by later
//! work and mounted into this window. See DESIGN.md.

use adw::prelude::*;
use gtk::glib;

/// Build the main window and present it.
pub fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Calculator")
        .default_width(380)
        .default_height(780)
        .width_request(300)
        .height_request(400)
        .build();

    // A simple toolbar view with a header bar and a centered placeholder body.
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let placeholder = gtk::Label::builder()
        .label("Calculator")
        .css_classes(["title-1"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .build();
    toolbar.set_content(Some(&placeholder));

    window.set_content(Some(&toolbar));

    // Phone-adaptive breakpoint at 550 sp (scaled pixels, respects the
    // text-scale factor). Below this width the window is in "phone" layout.
    // Registered here as an attachment point; per-widget setters are added by
    // later work.
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        550.0,
        adw::LengthUnit::Sp,
    ));
    window.add_breakpoint(bp);

    window.present();

    // Silence the unused-import lint until later work uses glib helpers here.
    let _ = glib::MainContext::default();
}
