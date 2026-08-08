//! The calculator window: display, basic + scientific keypads, history, memory.
//!
//! Everything is built in pure Rust (no `.ui` templates). A single
//! [`Ui`] handle owns the [`Calculator`] state machine (in an
//! `Rc<RefCell<_>>`) plus the widgets the [`Ui::render`] pass needs to touch, so
//! button closures and the hardware-keyboard controller all funnel through one
//! place. The display is drawn with non-editable [`gtk::Label`]s so the GNOME
//! on-screen keyboard never appears.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;
use gtk::{gdk, gio};

use crate::engine::AngleUnit;
use crate::history::{self, History};
use crate::settings;
use crate::state::{CalcState, Calculator, Func, Op};

/// Shared UI state. Cheap to `clone()` (it's all `Rc`/`RefCell` internally) so
/// it drops straight into `glib::clone!` closures.
#[derive(Clone, gtk::glib::Downgrade)]
pub struct Ui {
    calc: Rc<RefCell<Calculator>>,
    history: Rc<RefCell<History>>,
    /// The pretty expression line.
    expr_label: gtk::Label,
    /// The live-result / (Result state) primary answer line.
    result_label: gtk::Label,
    /// Persistent "DEG"/"RAD" and "M" indicators.
    indicator_label: gtk::Label,
    /// The Deg/Rad toggle in the scientific pad (its label tracks the mode).
    deg_button: gtk::Button,
    /// The Inv toggle in the scientific pad.
    inv_button: gtk::Button,
    /// The six scientific buttons whose labels flip in inverse mode.
    sci_sin: gtk::Button,
    sci_cos: gtk::Button,
    sci_tan: gtk::Button,
    sci_ln: gtk::Button,
    sci_log: gtk::Button,
    sci_sqrt: gtk::Button,
    /// The AdwNavigationView the history page is pushed onto.
    nav: adw::NavigationView,
}

impl Ui {
    /// Redraw the display from the calculator state. Called after every input.
    fn render(&self) {
        let calc = self.calc.borrow();

        // Reset transient classes; re-added below as the state demands.
        for w in [&self.expr_label, &self.result_label] {
            w.remove_css_class("calc-error");
            w.remove_css_class("calc-primary");
            w.remove_css_class("calc-secondary");
        }

        match calc.state() {
            CalcState::Error => {
                // Keep the (offending) expression up top, show the message big.
                self.expr_label.set_text(&calc.display_expression());
                self.expr_label.add_css_class("calc-secondary");
                self.expr_label.add_css_class("calc-error");
                let msg = calc.error_message().unwrap_or_default();
                self.result_label.set_text(&msg);
                self.result_label.set_visible(true);
                self.result_label.add_css_class("calc-primary");
                self.result_label.add_css_class("calc-error");
            }
            CalcState::Result => {
                // Swap emphasis: expression dims above, result is the big line.
                self.expr_label.set_text(&calc.display_expression());
                self.expr_label.add_css_class("calc-secondary");
                match calc.live_result().or_else(|| {
                    calc.current_value().map(crate::engine::format_result)
                }) {
                    Some(r) => {
                        self.result_label.set_text(&r);
                        self.result_label.set_visible(true);
                        self.result_label.add_css_class("calc-primary");
                    }
                    None => self.result_label.set_visible(false),
                }
            }
            CalcState::Input => {
                self.expr_label.set_text(&calc.display_expression());
                match calc.live_result() {
                    Some(r) => {
                        self.result_label.set_text(&r);
                        self.result_label.set_visible(true);
                    }
                    None => self.result_label.set_visible(false),
                }
            }
        }

        // DEG/RAD + memory indicator line.
        let mode = match calc.angle() {
            AngleUnit::Deg => "DEG",
            AngleUnit::Rad => "RAD",
        };
        let text = if calc.has_memory() {
            format!("M  {mode}")
        } else {
            mode.to_string()
        };
        self.indicator_label.set_text(&text);
    }

    /// Sync the scientific-pad toggles + labels to the calculator's inv/angle.
    fn sync_sci(&self) {
        let (inv, angle) = {
            let calc = self.calc.borrow();
            (calc.inv(), calc.angle())
        };

        // Inv active styling.
        if inv {
            self.inv_button.add_css_class("calc-active");
        } else {
            self.inv_button.remove_css_class("calc-active");
        }

        // Relabel the six inverse-form buttons.
        self.sci_sin.set_label(if inv { "sin\u{207B}\u{00B9}" } else { "sin" });
        self.sci_cos.set_label(if inv { "cos\u{207B}\u{00B9}" } else { "cos" });
        self.sci_tan.set_label(if inv { "tan\u{207B}\u{00B9}" } else { "tan" });
        self.sci_ln.set_label(if inv { "e\u{02E3}" } else { "ln" });
        self.sci_log.set_label(if inv { "10\u{02E3}" } else { "log" });
        self.sci_sqrt.set_label(if inv { "x\u{00B2}" } else { "\u{221A}" });

        // Deg/Rad button: show the CURRENT mode; mark active always (it's a
        // persistent mode indicator, like Google's).
        self.deg_button
            .set_label(match angle {
                AngleUnit::Deg => "Deg",
                AngleUnit::Rad => "Rad",
            });
        self.deg_button.add_css_class("calc-active");
    }

    /// Copy the current result/value to the clipboard.
    fn copy_result(&self) {
        let text = {
            let calc = self.calc.borrow();
            if !self.result_label.text().is_empty() && self.result_label.is_visible() {
                self.result_label.text().to_string()
            } else if let Some(v) = calc.current_value() {
                crate::engine::format_result(v)
            } else {
                calc.display_expression()
            }
        };
        if text.is_empty() {
            return;
        }
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
    }
}

/// Current wall-clock time in unix seconds.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build the main window and present it.
pub fn build_ui(app: &adw::Application) {
    let angle = settings::angle_mode();
    let calc = Rc::new(RefCell::new(Calculator::new(angle)));
    let history = Rc::new(RefCell::new(History::load()));

    let (width, height) = settings::window_size();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Calculator")
        .default_width(width)
        .default_height(height)
        .width_request(300)
        .height_request(400)
        .build();

    if settings::window_maximized() {
        window.maximize();
    }
    window.connect_close_request(|window| {
        let (w, h) = window.default_size();
        settings::set_window_size(w, h);
        settings::set_window_maximized(window.is_maximized());
        glib::Propagation::Proceed
    });

    // ── Display labels (non-editable — no OSK) ───────────────────────────
    let expr_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-expression"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();

    let result_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-result"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .visible(false)
        .build();
    result_label.set_selectable(false);

    let indicator_label = gtk::Label::builder()
        .label("RAD")
        .css_classes(["calc-indicator"])
        .halign(gtk::Align::Start)
        .build();

    // Scientific-pad buttons whose labels change; declared here so the Ui can
    // keep handles for relabeling.
    let deg_button = sci_button("Rad");
    let inv_button = sci_button("Inv");
    let sci_sin = sci_button("sin");
    let sci_cos = sci_button("cos");
    let sci_tan = sci_button("tan");
    let sci_ln = sci_button("ln");
    let sci_log = sci_button("log");
    let sci_sqrt = sci_button("\u{221A}");

    let nav = adw::NavigationView::new();

    let ui = Ui {
        calc: calc.clone(),
        history: history.clone(),
        expr_label: expr_label.clone(),
        result_label: result_label.clone(),
        indicator_label: indicator_label.clone(),
        deg_button: deg_button.clone(),
        inv_button: inv_button.clone(),
        sci_sin: sci_sin.clone(),
        sci_cos: sci_cos.clone(),
        sci_tan: sci_tan.clone(),
        sci_ln: sci_ln.clone(),
        sci_log: sci_log.clone(),
        sci_sqrt: sci_sqrt.clone(),
        nav: nav.clone(),
    };

    // ── Header: history (left), kebab (right) ────────────────────────────
    let header = adw::HeaderBar::new();
    header.set_show_title(false);

    let history_btn = gtk::Button::builder()
        .icon_name("document-open-recent-symbolic")
        .tooltip_text("History")
        .build();
    history_btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| show_history(&ui)
    ));
    header.pack_start(&history_btn);

    // Kebab menu (Copy / Clear history / About), backed by a gio::Menu model.
    let menu_model = gio::Menu::new();
    menu_model.append(Some("Copy result"), Some("calc.copy"));
    menu_model.append(Some("Clear history"), Some("calc.clear-history"));
    menu_model.append(Some("About Calculator"), Some("calc.about"));
    let kebab = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("Menu")
        .menu_model(&menu_model)
        .build();
    header.pack_end(&kebab);

    // ── Display box ──────────────────────────────────────────────────────
    let display = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(20)
        .margin_end(20)
        .margin_top(8)
        .margin_bottom(4)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    display.append(&indicator_label);
    display.append(&expr_label);
    display.append(&result_label);

    // Long-press + right-click on the result → memory/copy popover.
    attach_result_menu(&ui, &result_label);

    // ── Chevron handle (toggles the scientific revealer) ─────────────────
    let sci_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .reveal_child(false)
        .build();

    let chevron = gtk::Button::builder()
        .icon_name("pan-down-symbolic")
        .css_classes(["calc-chevron", "flat"])
        .halign(gtk::Align::Center)
        .hexpand(true)
        .build();
    chevron.connect_clicked(clone!(
        #[weak]
        sci_revealer,
        #[weak]
        chevron,
        move |_| {
            let open = !sci_revealer.reveals_child();
            sci_revealer.set_reveal_child(open);
            chevron.set_icon_name(if open {
                "pan-up-symbolic"
            } else {
                "pan-down-symbolic"
            });
        }
    ));

    // ── Scientific pad ───────────────────────────────────────────────────
    let sci_grid = build_scientific_pad(
        &ui, &deg_button, &inv_button, &sci_sin, &sci_cos, &sci_tan, &sci_ln,
        &sci_log, &sci_sqrt,
    );
    sci_revealer.set_child(Some(&sci_grid));

    // ── Basic pad ────────────────────────────────────────────────────────
    let basic_grid = build_basic_pad(&ui, app, &window);

    // ── Keypad column (scientific revealer above the basic grid) ─────────
    let keypad = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(16)
        .build();
    keypad.append(&sci_revealer);
    keypad.append(&basic_grid);

    // Keep the keypad phone-width on desktop.
    let keypad_clamp = adw::Clamp::builder()
        .maximum_size(440)
        .child(&keypad)
        .build();

    // ── Body: display (top), chevron, keypad (bottom) ────────────────────
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&display);
    body.append(&chevron);
    body.append(&keypad_clamp);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));

    let root_page = adw::NavigationPage::builder()
        .title("Calculator")
        .tag("calculator")
        .child(&toolbar)
        .build();
    nav.add(&root_page);

    window.set_content(Some(&nav));

    // Phone-adaptive breakpoint at 550 sp (registration point).
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        550.0,
        adw::LengthUnit::Sp,
    ));
    window.add_breakpoint(bp);

    // ── Kebab actions ────────────────────────────────────────────────────
    install_actions(app, &ui, &window);

    // ── Hardware keyboard ────────────────────────────────────────────────
    install_key_controller(&ui, &window);

    ui.sync_sci();
    ui.render();
    window.present();
}

/// Build a round basic-pad button of the given label + style class, wired to
/// call `on_press` then re-render.
fn key_button(label: &str, class: &str, ui: &Ui, on_press: impl Fn(&mut Calculator) + 'static) -> gtk::Button {
    let btn = gtk::Button::builder()
        .label(label)
        .css_classes(["calc-btn", class])
        .hexpand(true)
        .vexpand(true)
        .can_focus(false) // never steal focus from the window key controller
        .build();
    btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            on_press(&mut ui.calc.borrow_mut());
            ui.render();
        }
    ));
    btn
}

/// A bare scientific (pill) button. Wiring is added by the caller.
fn sci_button(label: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .css_classes(["calc-sci", "calc-function"])
        .hexpand(true)
        .vexpand(true)
        .can_focus(false)
        .build()
}

/// Wire an already-built scientific button to `on_press`, capturing the Ui.
fn wire_sci(btn: &gtk::Button, ui: &Ui, on_press: impl Fn(&mut Calculator) + 'static) {
    btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            on_press(&mut ui.calc.borrow_mut());
            ui.render();
        }
    ));
}

/// The 5×4 basic keypad grid.
#[allow(clippy::too_many_arguments)]
fn build_basic_pad(ui: &Ui, _app: &adw::Application, _window: &adw::ApplicationWindow) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(8)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .build();

    // Row 0: AC ( ) % ÷
    grid.attach(&key_button("AC", "calc-clear", ui, |c| c.clear()), 0, 0, 1, 1);
    grid.attach(&key_button("( )", "calc-operator", ui, |c| c.press_paren()), 1, 0, 1, 1);
    grid.attach(&key_button("%", "calc-operator", ui, |c| c.press_percent()), 2, 0, 1, 1);
    grid.attach(&key_button("\u{00F7}", "calc-operator", ui, |c| c.press_op(Op::Div)), 3, 0, 1, 1);

    // Row 1: 7 8 9 ×
    grid.attach(&key_button("7", "calc-digit", ui, |c| c.press_digit('7')), 0, 1, 1, 1);
    grid.attach(&key_button("8", "calc-digit", ui, |c| c.press_digit('8')), 1, 1, 1, 1);
    grid.attach(&key_button("9", "calc-digit", ui, |c| c.press_digit('9')), 2, 1, 1, 1);
    grid.attach(&key_button("\u{00D7}", "calc-operator", ui, |c| c.press_op(Op::Mul)), 3, 1, 1, 1);

    // Row 2: 4 5 6 −
    grid.attach(&key_button("4", "calc-digit", ui, |c| c.press_digit('4')), 0, 2, 1, 1);
    grid.attach(&key_button("5", "calc-digit", ui, |c| c.press_digit('5')), 1, 2, 1, 1);
    grid.attach(&key_button("6", "calc-digit", ui, |c| c.press_digit('6')), 2, 2, 1, 1);
    grid.attach(&key_button("\u{2212}", "calc-operator", ui, |c| c.press_op(Op::Sub)), 3, 2, 1, 1);

    // Row 3: 1 2 3 +
    grid.attach(&key_button("1", "calc-digit", ui, |c| c.press_digit('1')), 0, 3, 1, 1);
    grid.attach(&key_button("2", "calc-digit", ui, |c| c.press_digit('2')), 1, 3, 1, 1);
    grid.attach(&key_button("3", "calc-digit", ui, |c| c.press_digit('3')), 2, 3, 1, 1);
    grid.attach(&key_button("+", "calc-operator", ui, |c| c.press_op(Op::Add)), 3, 3, 1, 1);

    // Row 4: 0 . ⌫ =
    grid.attach(&key_button("0", "calc-digit", ui, |c| c.press_digit('0')), 0, 4, 1, 1);
    grid.attach(&key_button(".", "calc-digit", ui, |c| c.press_dot()), 1, 4, 1, 1);

    // Backspace: an icon button that still uses the round key styling.
    let back = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .css_classes(["calc-btn", "calc-function"])
        .hexpand(true)
        .vexpand(true)
        .can_focus(false)
        .build();
    back.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            ui.calc.borrow_mut().backspace();
            ui.render();
        }
    ));
    grid.attach(&back, 2, 4, 1, 1);

    // Equals: commit + persist history.
    let equals = gtk::Button::builder()
        .label("=")
        .css_classes(["calc-btn", "calc-equals"])
        .hexpand(true)
        .vexpand(true)
        .can_focus(false)
        .build();
    equals.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| do_equals(&ui)
    ));
    grid.attach(&equals, 3, 4, 1, 1);

    grid
}

/// Commit the current expression, persist any new history entry, and re-render.
fn do_equals(ui: &Ui) {
    let entry = ui.calc.borrow_mut().equals();
    if let Some(entry) = entry {
        let mut hist = ui.history.borrow_mut();
        hist.push(entry);
        hist.save();
    }
    ui.render();
}

/// The 3×4 scientific keypad grid (revealed above the basic pad).
#[allow(clippy::too_many_arguments)]
fn build_scientific_pad(
    ui: &Ui,
    deg: &gtk::Button,
    inv: &gtk::Button,
    sin: &gtk::Button,
    cos: &gtk::Button,
    tan: &gtk::Button,
    ln: &gtk::Button,
    log: &gtk::Button,
    sqrt: &gtk::Button,
) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(6)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .margin_bottom(4)
        .build();

    // Row 0: √ π ^ !
    wire_sci(sqrt, ui, |c| c.press_sqrt());
    let pi = sci_button("\u{03C0}");
    wire_sci(&pi, ui, |c| c.press_pi());
    let pow = sci_button("^");
    wire_sci(&pow, ui, |c| c.press_power());
    let fact = sci_button("!");
    wire_sci(&fact, ui, |c| c.press_factorial());
    grid.attach(sqrt, 0, 0, 1, 1);
    grid.attach(&pi, 1, 0, 1, 1);
    grid.attach(&pow, 2, 0, 1, 1);
    grid.attach(&fact, 3, 0, 1, 1);

    // Row 1: Deg sin cos tan
    deg.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            let next = match ui.calc.borrow().angle() {
                AngleUnit::Deg => AngleUnit::Rad,
                AngleUnit::Rad => AngleUnit::Deg,
            };
            ui.calc.borrow_mut().set_angle(next);
            settings::set_angle_mode(next);
            ui.sync_sci();
            ui.render();
        }
    ));
    wire_sci(sin, ui, |c| c.press_func(Func::Sin));
    wire_sci(cos, ui, |c| c.press_func(Func::Cos));
    wire_sci(tan, ui, |c| c.press_func(Func::Tan));
    grid.attach(deg, 0, 1, 1, 1);
    grid.attach(sin, 1, 1, 1, 1);
    grid.attach(cos, 2, 1, 1, 1);
    grid.attach(tan, 3, 1, 1, 1);

    // Row 2: Inv e ln log
    inv.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            ui.calc.borrow_mut().toggle_inv();
            ui.sync_sci();
            ui.render();
        }
    ));
    let euler = sci_button("e");
    wire_sci(&euler, ui, |c| c.press_e());
    wire_sci(ln, ui, |c| c.press_func(Func::Ln));
    wire_sci(log, ui, |c| c.press_func(Func::Log));
    grid.attach(inv, 0, 2, 1, 1);
    grid.attach(&euler, 1, 2, 1, 1);
    grid.attach(ln, 2, 2, 1, 1);
    grid.attach(log, 3, 2, 1, 1);

    grid
}

/// Install the kebab-menu GActions (`calc.copy`, `calc.clear-history`,
/// `calc.about`) into an action group on the window.
fn install_actions(app: &adw::Application, ui: &Ui, window: &adw::ApplicationWindow) {
    let group = gio::SimpleActionGroup::new();

    let copy = gio::SimpleAction::new("copy", None);
    copy.connect_activate(clone!(
        #[weak]
        ui,
        move |_, _| ui.copy_result()
    ));
    group.add_action(&copy);

    let clear = gio::SimpleAction::new("clear-history", None);
    clear.connect_activate(clone!(
        #[weak]
        ui,
        move |_, _| {
            let mut hist = ui.history.borrow_mut();
            hist.clear();
            hist.save();
        }
    ));
    group.add_action(&clear);

    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(clone!(
        #[weak]
        window,
        move |_, _| present_about(&window)
    ));
    group.add_action(&about);

    window.insert_action_group("calc", Some(&group));
    let _ = app; // app kept in signature for parity with the house pattern
}

/// The About dialog.
fn present_about(window: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Calculator")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Daniel Miller")
        .license_type(gtk::License::Gpl30)
        .comments(
            "A native GTK4/libadwaita calculator — Google Calculator-style, \
             mobile-first for GNOME Shell Mobile, adapting to your theme and \
             accent color.",
        )
        .build();
    about.present(Some(window));
}

/// Attach a long-press + right-click popover offering Copy and memory ops to the
/// result label.
fn attach_result_menu(ui: &Ui, result_label: &gtk::Label) {
    let long = gtk::GestureLongPress::new();
    long.connect_pressed(clone!(
        #[weak]
        ui,
        #[weak]
        result_label,
        move |_, x, y| show_result_popover(&ui, result_label.upcast_ref(), x, y)
    ));
    result_label.add_controller(long);

    let right = gtk::GestureClick::new();
    right.set_button(gdk::BUTTON_SECONDARY);
    right.connect_pressed(clone!(
        #[weak]
        ui,
        #[weak]
        result_label,
        move |_, _, x, y| show_result_popover(&ui, result_label.upcast_ref(), x, y)
    ));
    result_label.add_controller(right);

    // The label must accept pointer events for the gestures to fire.
    result_label.set_selectable(false);
}

/// Pop a small memory/copy menu anchored at (x, y) over `anchor`.
fn show_result_popover(ui: &Ui, anchor: &gtk::Widget, x: f64, y: f64) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();

    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(true)
        .build();
    // Tear down any popover still parented on the anchor from a previous open.
    let mut child = anchor.first_child();
    while let Some(w) = child {
        let next = w.next_sibling();
        if let Some(p) = w.downcast_ref::<gtk::Popover>() {
            p.popdown();
            p.unparent();
        }
        child = next;
    }
    popover.set_parent(anchor);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let add_row = |label: &str, cb: Box<dyn Fn()>| -> gtk::Button {
        let b = gtk::Button::builder()
            .label(label)
            .css_classes(["flat"])
            .halign(gtk::Align::Fill)
            .build();
        b.connect_clicked(move |_| cb());
        b
    };

    // Copy — always.
    {
        let ui2 = ui.clone();
        let pop = popover.clone();
        content.append(&add_row(
            "Copy",
            Box::new(move || {
                ui2.copy_result();
                pop.popdown();
            }),
        ));
    }

    // MS — store current value.
    {
        let ui2 = ui.clone();
        let pop = popover.clone();
        content.append(&add_row(
            "MS",
            Box::new(move || {
                ui2.calc.borrow_mut().memory_store();
                ui2.render();
                pop.popdown();
            }),
        ));
    }

    if ui.calc.borrow().has_memory() {
        for (label, action) in [
            ("MR", 0u8),
            ("M+", 1),
            ("M\u{2212}", 2),
            ("MC", 3),
        ] {
            let ui2 = ui.clone();
            let pop = popover.clone();
            content.append(&add_row(
                label,
                Box::new(move || {
                    {
                        let mut c = ui2.calc.borrow_mut();
                        match action {
                            0 => c.memory_recall(),
                            1 => c.memory_add(),
                            2 => c.memory_sub(),
                            _ => c.memory_clear(),
                        }
                    }
                    ui2.render();
                    pop.popdown();
                }),
            ));
        }
    }

    popover.set_child(Some(&content));
    // Free the popover once it's dismissed so we don't leak parents on the label.
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}

/// Build and push the history navigation page.
fn show_history(ui: &Ui) {
    let now = now_unix();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();

    let hist = ui.history.borrow();
    if hist.is_empty() {
        let empty = adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .title("No history yet")
            .description("Completed calculations will appear here.")
            .vexpand(true)
            .build();
        content.append(&empty);
    } else {
        // Entries are stored oldest-first; show newest-first, grouped by day.
        let entries: Vec<_> = hist.entries().iter().rev().cloned().collect();
        let mut current_label: Option<String> = None;
        let mut group: Option<adw::PreferencesGroup> = None;

        for entry in &entries {
            let label = history::day_label(entry.timestamp, now);
            if current_label.as_deref() != Some(label.as_str()) {
                let g = adw::PreferencesGroup::builder().title(&label).build();
                content.append(&g);
                group = Some(g);
                current_label = Some(label);
            }

            let row = adw::ActionRow::builder().activatable(true).build();

            // Two-line row: dim expression above, larger result below.
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .halign(gtk::Align::End)
                .hexpand(true)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            let expr = gtk::Label::builder()
                .label(&entry.expression)
                .css_classes(["calc-hist-expr"])
                .halign(gtk::Align::End)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::Start)
                .build();
            let res = gtk::Label::builder()
                .label(format!("= {}", entry.result))
                .css_classes(["calc-hist-result"])
                .halign(gtk::Align::End)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::Start)
                .build();
            text.append(&expr);
            text.append(&res);
            row.add_suffix(&text);

            // Tap → insert the result into the current expression, pop back.
            let result_value = entry.result.clone();
            row.connect_activated(clone!(
                #[weak]
                ui,
                move |_| {
                    ui.calc.borrow_mut().insert_result(&result_value);
                    ui.render();
                    ui.nav.pop();
                }
            ));

            if let Some(g) = &group {
                g.add(&row);
            }
        }
    }
    drop(hist);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();

    let clamp = adw::Clamp::builder()
        .maximum_size(520)
        .child(&scroller)
        .build();

    let header = adw::HeaderBar::new();
    let clear_btn = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Clear history")
        .build();
    clear_btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            {
                let mut h = ui.history.borrow_mut();
                h.clear();
                h.save();
            }
            // Pop and re-open so the (now empty) view refreshes.
            ui.nav.pop();
            show_history(&ui);
        }
    ));
    header.pack_end(&clear_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&clamp));

    let page = adw::NavigationPage::builder()
        .title("History")
        .tag("history")
        .child(&toolbar)
        .build();
    ui.nav.push(&page);
}

/// Install the window-level hardware-keyboard controller. Keys map to the same
/// calculator methods the on-screen buttons call. Buttons are `can_focus(false)`
/// so this controller always sees the input.
fn install_key_controller(ui: &Ui, window: &adw::ApplicationWindow) {
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed(clone!(
        // NOTE: kept #[strong] — returns glib::Propagation (no default) and is tied to the window lifetime.
        #[strong]
        ui,
        move |_, keyval, _keycode, _modifier| {
            // Only act on the calculator page; let the history page handle its
            // own navigation (Escape = back, etc.).
            if ui.nav.visible_page().and_then(|p| p.tag()).as_deref() != Some("calculator") {
                return glib::Propagation::Proceed;
            }

            let mut handled = true;
            {
                let mut calc = ui.calc.borrow_mut();
                if let Some(ch) = keyval.to_unicode() {
                    match ch {
                        '0'..='9' => calc.press_digit(ch),
                        '.' | ',' => calc.press_dot(),
                        '+' => calc.press_op(Op::Add),
                        '-' => calc.press_op(Op::Sub),
                        '*' => calc.press_op(Op::Mul),
                        '/' => calc.press_op(Op::Div),
                        '^' => calc.press_power(),
                        '%' => calc.press_percent(),
                        '!' => calc.press_factorial(),
                        '(' | ')' => calc.press_paren(),
                        '=' => {
                            drop(calc);
                            do_equals(&ui);
                            return glib::Propagation::Stop;
                        }
                        'p' => calc.press_pi(),
                        's' => calc.press_func(Func::Sin),
                        'c' => calc.press_func(Func::Cos),
                        't' => calc.press_func(Func::Tan),
                        _ => handled = false,
                    }
                } else {
                    handled = false;
                }
            }

            if !handled {
                // Named keys (Enter/Backspace/Escape/Delete) have no unicode.
                match keyval {
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        do_equals(&ui);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::BackSpace => {
                        ui.calc.borrow_mut().backspace();
                    }
                    gdk::Key::Escape | gdk::Key::Delete => {
                        ui.calc.borrow_mut().clear();
                    }
                    _ => return glib::Propagation::Proceed,
                }
            }

            ui.render();
            glib::Propagation::Stop
        }
    ));
    window.add_controller(controller);
}
