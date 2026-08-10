//! Calculator — a native GTK4/libadwaita calculator, Google Calculator-style,
//! mobile-first for GNOME Shell Mobile (aarch64) and scaling to the desktop.
//!
//! Application entry point: initialises logging and libadwaita, installs the
//! stylesheet on startup and hands control to [`app::build_ui`] on activation.
//! The evaluation engine, calculator state machine and full UI are built by
//! later work; this file wires up the application shell. See DESIGN.md.

mod app;
mod convert;
mod engine;
mod history;
mod settings;
mod state;
mod ui;

use gtk::prelude::*;

/// Reverse-DNS application id. Also the GSettings schema id and the D-Bus name;
/// keep it in sync with `data/*.gschema.xml` and the `.desktop` file.
pub const APP_ID: &str = "io.matv.Calculator";

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "calculator=info,warn".into()),
        )
        .init();

    adw::init().expect("failed to initialise libadwaita");

    let application = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    application.connect_startup(|_| {
        ui::load_css();
    });

    application.connect_activate(app::build_ui);

    // We manage our own args (none of interest); pass an empty slice so GTK
    // doesn't try to parse cargo/test flags.
    application.run_with_args::<&str>(&[])
}
