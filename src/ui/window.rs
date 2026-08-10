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
    /// Holds both orientation copies (portrait then landscape).
    deg_button: Rc<Vec<gtk::Button>>,
    /// The Inv toggle in the scientific pad.
    /// Holds both orientation copies (portrait then landscape).
    inv_button: Rc<Vec<gtk::Button>>,
    /// The six scientific buttons whose labels flip in inverse mode.
    /// Each holds both orientation copies (portrait then landscape).
    sci_sin: Rc<Vec<gtk::Button>>,
    sci_cos: Rc<Vec<gtk::Button>>,
    sci_tan: Rc<Vec<gtk::Button>>,
    sci_ln: Rc<Vec<gtk::Button>>,
    sci_log: Rc<Vec<gtk::Button>>,
    sci_sqrt: Rc<Vec<gtk::Button>>,
    /// The AdwNavigationView the history page is pushed onto.
    nav: adw::NavigationView,
    /// Converter page state (category + selected unit indices + input string).
    /// A plain Rc<RefCell<>> holder, entirely separate from the Calculator
    /// state machine. Reset each time the converter page is opened.
    converter: Rc<RefCell<ConverterState>>,
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
                        self.result_label.add_css_class("calc-primary");
                    }
                    None => self.result_label.set_text(""),
                }
            }
            CalcState::Input => {
                self.expr_label.set_text(&calc.display_expression());
                match calc.live_result() {
                    Some(r) => {
                        self.result_label.set_text(&r);
                    }
                    None => self.result_label.set_text(""),
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

        // Inv active styling (every orientation copy).
        for b in self.inv_button.iter() {
            if inv {
                b.add_css_class("calc-active");
            } else {
                b.remove_css_class("calc-active");
            }
        }

        // Relabel the six inverse-form buttons (every orientation copy).
        for b in self.sci_sin.iter() {
            b.set_label(if inv { "sin\u{207B}\u{00B9}" } else { "sin" });
        }
        for b in self.sci_cos.iter() {
            b.set_label(if inv { "cos\u{207B}\u{00B9}" } else { "cos" });
        }
        for b in self.sci_tan.iter() {
            b.set_label(if inv { "tan\u{207B}\u{00B9}" } else { "tan" });
        }
        for b in self.sci_ln.iter() {
            b.set_label(if inv { "e\u{02E3}" } else { "ln" });
        }
        for b in self.sci_log.iter() {
            b.set_label(if inv { "10\u{02E3}" } else { "log" });
        }
        for b in self.sci_sqrt.iter() {
            b.set_label(if inv { "x\u{00B2}" } else { "\u{221A}" });
        }

        // Deg/Rad button: show the CURRENT mode. It's a normal gray
        // scientific button — the mode is communicated by its label ("Deg" /
        // "Rad") and the DEG/RAD display indicator, not by an accent highlight.
        for b in self.deg_button.iter() {
            b.set_label(match angle {
                AngleUnit::Deg => "Deg",
                AngleUnit::Rad => "Rad",
            });
            b.remove_css_class("calc-active");
        }
    }

    /// Copy the current result/value to the clipboard.
    fn copy_result(&self) {
        let text = {
            let calc = self.calc.borrow();
            if !self.result_label.text().is_empty() {
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

/// Converter page state — deliberately separate from the `Calculator` state
/// machine. `from_idx`/`to_idx` index into `category.units()`. `input` is the
/// raw decimal string the reduced keypad builds ("" / "-" / "." parse to 0.0).
struct ConverterState {
    category: crate::convert::Category,
    from_idx: usize,
    to_idx: usize,
    input: String,
}

impl ConverterState {
    /// Parse the input string to f64; empty / "-" / "." / "-." → 0.0.
    fn value(&self) -> f64 {
        self.input.parse::<f64>().unwrap_or(0.0)
    }
}

/// Index of the unit with `id` within `cat.units()`, or 0 if not found.
fn category_index_of(cat: crate::convert::Category, id: &str) -> usize {
    cat.units().iter().position(|u| u.id == id).unwrap_or(0)
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
    let start_cat = settings::converter_category();

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
        .build();
    result_label.set_selectable(false);

    let indicator_label = gtk::Label::builder()
        .label("RAD")
        .css_classes(["calc-indicator"])
        .halign(gtk::Align::Start)
        .build();

    let nav = adw::NavigationView::new();

    // Two independent orientation button sets — portrait (sp) and landscape
    // (sl). Built BEFORE `ui` so the Vecs can be populated from clones and are
    // fully live before any closure or sync_sci runs.
    let sp = make_sci_buttons();
    let sl = make_sci_buttons();

    let ui = Ui {
        calc: calc.clone(),
        history: history.clone(),
        expr_label: expr_label.clone(),
        result_label: result_label.clone(),
        indicator_label: indicator_label.clone(),
        // Portrait copy first, landscape copy second (fixed order).
        deg_button: Rc::new(vec![sp.deg.clone(), sl.deg.clone()]),
        inv_button: Rc::new(vec![sp.inv.clone(), sl.inv.clone()]),
        sci_sin: Rc::new(vec![sp.sin.clone(), sl.sin.clone()]),
        sci_cos: Rc::new(vec![sp.cos.clone(), sl.cos.clone()]),
        sci_tan: Rc::new(vec![sp.tan.clone(), sl.tan.clone()]),
        sci_ln: Rc::new(vec![sp.ln.clone(), sl.ln.clone()]),
        sci_log: Rc::new(vec![sp.log.clone(), sl.log.clone()]),
        sci_sqrt: Rc::new(vec![sp.sqrt.clone(), sl.sqrt.clone()]),
        nav: nav.clone(),
        converter: Rc::new(RefCell::new(ConverterState {
            category: start_cat,
            from_idx: category_index_of(start_cat, start_cat.default_from().id),
            to_idx: category_index_of(start_cat, start_cat.default_to().id),
            input: String::new(),
        })),
    };

    // Wire both stateful button sets (each set wired exactly once — no widget
    // is double-connected).
    wire_sci_buttons(&ui, &sp);
    wire_sci_buttons(&ui, &sl);

    // Apply the persisted inverse mode BEFORE the first sync/render, using the
    // top-of-fn `calc` local (no active borrow conflict here).
    calc.borrow_mut().set_inv(settings::inverse_mode());

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

    let converter_btn = gtk::Button::builder()
        .icon_name("object-flip-vertical-symbolic")
        .tooltip_text("Unit converter")
        .build();
    converter_btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| show_converter(&ui)
    ));
    header.pack_start(&converter_btn);

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
        .css_classes(["calc-display"])
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

    // Pull-down-to-reveal: a downward drag on the display area opens history.
    // Kept on the display BOX (parent) so the result_label's own long-press /
    // right-click still win for stationary gestures; we only Claim once the drag
    // is clearly vertical + downward.
    let pull = gtk::GestureDrag::new();
    pull.connect_drag_update(move |g, off_x, off_y| {
        // Predominantly downward past a small dead-zone → take the sequence.
        if off_y > 12.0 && off_y.abs() > off_x.abs() {
            g.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    pull.connect_drag_end(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_g, off_x, off_y| {
            // Commit if the drag traveled far enough downward and stayed vertical.
            if off_y > 80.0 && off_y.abs() > off_x.abs() {
                show_history(&ui);
            }
        }
    ));
    display.add_controller(pull);

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

    // ── Scientific + basic pads (one instance per orientation page) ──────
    let sci_grid_portrait = build_scientific_pad_portrait(&ui, &sp);
    let sci_grid_landscape = build_scientific_pad_landscape(&ui, &sl);
    let basic_portrait = build_basic_pad(&ui, app, &window);
    let basic_landscape = build_basic_pad(&ui, app, &window);

    // Landscape basic pad: give every button a shrink-friendly class + let them
    // fill the row height. (Portrait's basic pad is a separate instance, so this
    // never touches the portrait keypad's fixed-height stability.)
    basic_landscape.set_vexpand(true);
    {
        let mut child = basic_landscape.first_child();
        while let Some(w) = child {
            if let Some(b) = w.downcast_ref::<gtk::Button>() {
                b.add_css_class("calc-btn-land");
                b.set_vexpand(true);
            }
            child = w.next_sibling();
        }
    }

    // The portrait scientific pad lives behind the chevron revealer.
    sci_revealer.set_child(Some(&sci_grid_portrait));

    // ── Keypad stack: two orientation pages sharing one display + state ──
    // "portrait"  page: chevron + (sci_revealer over basic grid) — default child.
    // "landscape" page: 3-col scientific grid inline left of 4-col basic grid.
    // The aspect-ratio breakpoint (bp_landscape) flips visible-child-name to
    // "landscape" when width>=height; otherwise it reverts to the default
    // "portrait" child. Each page owns independent button instances; the 8
    // stateful sci buttons live in the Ui Vecs (deg_button/inv_button/sci_*),
    // one entry per page, and sync_sci relabels every copy.

    // Portrait page: chevron above the (revealer-over-basic) keypad column.
    let portrait_keypad = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(16)
        .vexpand(false)
        .valign(gtk::Align::End)
        .build();
    portrait_keypad.append(&sci_revealer);
    portrait_keypad.append(&basic_portrait);

    let portrait_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    portrait_page.append(&chevron);
    portrait_page.append(&portrait_keypad);

    // Landscape page: scientific grid inline to the left of the basic grid.
    sci_grid_landscape.set_hexpand(true);
    basic_landscape.set_hexpand(true);
    let landscape_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(16)
        .valign(gtk::Align::End)
        .build();
    landscape_page.append(&sci_grid_landscape);
    landscape_page.append(&basic_landscape);

    let keypad_stack = gtk::Stack::new();
    keypad_stack.set_hhomogeneous(false);
    keypad_stack.set_vhomogeneous(false);
    keypad_stack.add_named(&portrait_page, Some("portrait"));
    keypad_stack.add_named(&landscape_page, Some("landscape"));
    keypad_stack.set_visible_child_name("portrait");

    // Keep the keypad width-bounded on desktop. Raised from 440 → 760 so the
    // wider landscape (sci + basic side by side) isn't cramped.
    let keypad_clamp = adw::Clamp::builder()
        .maximum_size(960)
        .child(&keypad_stack)
        .vexpand(false)
        .valign(gtk::Align::End)
        .build();

    // ── Body: display (top), keypad stack (bottom) ───────────────────────
    // The chevron now lives inside the portrait page, so it is NOT appended
    // to the body here.
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&display);
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

    // Landscape when aspect ratio >= 1/1 (width >= height): show inline sci grid.
    let cond = adw::BreakpointCondition::parse("min-aspect-ratio: 1/1")
        .unwrap_or_else(|_| adw::BreakpointCondition::new_ratio(
            adw::BreakpointConditionRatioType::MinAspectRatio, 1, 1));
    let bp_landscape = adw::Breakpoint::new(cond);
    bp_landscape.add_setter(&keypad_stack, "visible-child-name", Some(&"landscape".to_value()));
    // Landscape: shrink the shared display to a compact top strip and let the
    // keypad take the freed vertical space. valign is flipped to Fill so the
    // vexpanding, row-homogeneous grids stretch to divide the height evenly
    // (End/Center would keep them short at natural height). GTK restores the
    // pre-apply values (display vexpand=true/valign=Center, keypad_stack
    // vexpand=false, keypad_clamp vexpand=false/valign=End, landscape_page
    // valign=End) on unapply, so portrait is unchanged.
    bp_landscape.add_setter(&display, "vexpand", Some(&false.to_value()));
    bp_landscape.add_setter(&display, "valign", Some(&gtk::Align::Start.to_value()));
    bp_landscape.add_setter(&display, "height-request", Some(&110i32.to_value()));
    bp_landscape.add_setter(&display, "css-classes", Some(&vec!["calc-display", "landscape"].to_value()));
    bp_landscape.add_setter(&keypad_stack, "vexpand", Some(&true.to_value()));
    bp_landscape.add_setter(&keypad_clamp, "vexpand", Some(&true.to_value()));
    bp_landscape.add_setter(&keypad_clamp, "valign", Some(&gtk::Align::Fill.to_value()));
    bp_landscape.add_setter(&landscape_page, "valign", Some(&gtk::Align::Fill.to_value()));
    bp_landscape.add_setter(&landscape_page, "vexpand", Some(&true.to_value()));
    bp_landscape.add_setter(&landscape_page, "margin-bottom", Some(&24i32.to_value()));
    window.add_breakpoint(bp_landscape);

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
        .vexpand(false)
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
        .vexpand(false)
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

/// One fresh set of the 8 stateful scientific buttons — one such set exists per
/// orientation page. Returned in a fixed order so the caller can push each into
/// the matching Ui Vec. Labels/styling are applied later by `sync_sci`.
struct SciButtons {
    deg: gtk::Button,
    inv: gtk::Button,
    sin: gtk::Button,
    cos: gtk::Button,
    tan: gtk::Button,
    ln: gtk::Button,
    log: gtk::Button,
    sqrt: gtk::Button,
}

/// Build a fresh set of the 8 stateful scientific buttons (deg, inv, sin, cos,
/// tan, ln, log, sqrt). Initial labels are placeholders; `sync_sci` relabels
/// every copy to reflect the current inv/angle state.
fn make_sci_buttons() -> SciButtons {
    SciButtons {
        deg: sci_button("Rad"),
        inv: sci_button("Inv"),
        sin: sci_button("sin"),
        cos: sci_button("cos"),
        tan: sci_button("tan"),
        ln: sci_button("ln"),
        log: sci_button("log"),
        sqrt: sci_button("\u{221A}"),
    }
}

/// Wire all 8 stateful scientific buttons of one `SciButtons` set to the Ui.
/// Each page owns its own set, so this is called once per set — no widget is
/// double-connected. The deg/inv handlers keep the same borrow discipline as
/// the rest of the app: read the current state in a short-lived immutable
/// borrow, drop it, then mutate and re-read for persistence.
fn wire_sci_buttons(ui: &Ui, s: &SciButtons) {
    // Deg/Rad toggle: read current angle (borrow dropped), then flip + persist.
    s.deg.connect_clicked(clone!(
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

    // Inv toggle: mutate, then re-read with a FRESH borrow for persistence so
    // the mut borrow is never held across the `.inv()` read.
    s.inv.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| {
            ui.calc.borrow_mut().toggle_inv();
            ui.sync_sci();
            ui.render();
            let v = ui.calc.borrow().inv();
            settings::set_inverse_mode(v);
        }
    ));

    wire_sci(&s.sin, ui, |c| c.press_func(Func::Sin));
    wire_sci(&s.cos, ui, |c| c.press_func(Func::Cos));
    wire_sci(&s.tan, ui, |c| c.press_func(Func::Tan));
    wire_sci(&s.ln, ui, |c| c.press_func(Func::Ln));
    wire_sci(&s.log, ui, |c| c.press_func(Func::Log));
    wire_sci(&s.sqrt, ui, |c| c.press_sqrt());
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
        .vexpand(false)
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
        .vexpand(false)
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

/// The portrait 4-col scientific keypad grid (revealed above the basic pad).
///
/// Only the STATELESS buttons (π, ^, !, e) are created and wired here; the 8
/// stateful buttons in `s` are wired separately by `wire_sci_buttons`. This
/// builder just attaches every button to the grid.
fn build_scientific_pad_portrait(ui: &Ui, s: &SciButtons) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(6)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .margin_bottom(4)
        .build();

    // Stateless, page-local buttons.
    let pi = sci_button("\u{03C0}");
    wire_sci(&pi, ui, |c| c.press_pi());
    let pow = sci_button("^");
    wire_sci(&pow, ui, |c| c.press_power());
    let fact = sci_button("!");
    wire_sci(&fact, ui, |c| c.press_factorial());
    let euler = sci_button("e");
    wire_sci(&euler, ui, |c| c.press_e());

    // Row 0: √ π ^ !
    grid.attach(&s.sqrt, 0, 0, 1, 1);
    grid.attach(&pi, 1, 0, 1, 1);
    grid.attach(&pow, 2, 0, 1, 1);
    grid.attach(&fact, 3, 0, 1, 1);

    // Row 1: Deg sin cos tan
    grid.attach(&s.deg, 0, 1, 1, 1);
    grid.attach(&s.sin, 1, 1, 1, 1);
    grid.attach(&s.cos, 2, 1, 1, 1);
    grid.attach(&s.tan, 3, 1, 1, 1);

    // Row 2: Inv e ln log
    grid.attach(&s.inv, 0, 2, 1, 1);
    grid.attach(&euler, 1, 2, 1, 1);
    grid.attach(&s.ln, 2, 2, 1, 1);
    grid.attach(&s.log, 3, 2, 1, 1);

    grid
}

/// The landscape 3-col scientific keypad grid (shown inline left of the basic
/// pad when width ≥ height).
///
/// Same contract as the portrait builder: only the STATELESS buttons (π, ^, !,
/// e) are created + wired here; the 8 stateful buttons in `s` are wired by
/// `wire_sci_buttons`. This builder just attaches every button to the grid.
fn build_scientific_pad_landscape(ui: &Ui, s: &SciButtons) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(6)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Stateless, page-local buttons.
    let pi = sci_button("\u{03C0}");
    wire_sci(&pi, ui, |c| c.press_pi());
    pi.add_css_class("calc-sci-land");
    pi.set_vexpand(true);
    let pow = sci_button("^");
    wire_sci(&pow, ui, |c| c.press_power());
    pow.add_css_class("calc-sci-land");
    pow.set_vexpand(true);
    let fact = sci_button("!");
    wire_sci(&fact, ui, |c| c.press_factorial());
    fact.add_css_class("calc-sci-land");
    fact.set_vexpand(true);
    let euler = sci_button("e");
    wire_sci(&euler, ui, |c| c.press_e());
    euler.add_css_class("calc-sci-land");
    euler.set_vexpand(true);

    // Landscape sci buttons: shrink class + fill row height.
    for b in [&s.inv, &s.deg, &s.sqrt, &s.sin, &s.ln, &s.cos, &s.log, &s.tan] {
        b.add_css_class("calc-sci-land");
        b.set_vexpand(true);
    }

    // Row 0: Inv Deg √
    grid.attach(&s.inv, 0, 0, 1, 1);
    grid.attach(&s.deg, 1, 0, 1, 1);
    grid.attach(&s.sqrt, 2, 0, 1, 1);

    // Row 1: sin ln π
    grid.attach(&s.sin, 0, 1, 1, 1);
    grid.attach(&s.ln, 1, 1, 1, 1);
    grid.attach(&pi, 2, 1, 1, 1);

    // Row 2: cos log ^
    grid.attach(&s.cos, 0, 2, 1, 1);
    grid.attach(&s.log, 1, 2, 1, 1);
    grid.attach(&pow, 2, 2, 1, 1);

    // Row 3: tan e !
    grid.attach(&s.tan, 0, 3, 1, 1);
    grid.attach(&euler, 1, 3, 1, 1);
    grid.attach(&fact, 2, 3, 1, 1);

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

    let converter = gio::SimpleAction::new("converter", None);
    converter.connect_activate(clone!(
        #[weak]
        ui,
        move |_, _| show_converter(&ui)
    ));
    group.add_action(&converter);

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
        let entries_len = entries.len();
        let mut current_label: Option<String> = None;
        let mut group: Option<adw::PreferencesGroup> = None;

        for (display_i, entry) in entries.iter().enumerate() {
            // Display is newest-first (reversed); map back to the storage index
            // (oldest-first) so History::remove targets the right entry.
            let storage_idx = entries_len - 1 - display_i;

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

            // Swipe-to-delete: wrap the row in an Overlay whose background is a
            // red trash strip. The foreground is an opaque single-row ListBox
            // (ActionRow alone is transparent) that we fade/nudge left as the
            // drag progresses, and delete past a threshold.
            let fg_list = gtk::ListBox::new();
            fg_list.add_css_class("boxed-list");
            fg_list.set_selection_mode(gtk::SelectionMode::None);
            fg_list.append(&row);

            let del_strip = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .hexpand(true)
                .vexpand(true)
                .build();
            del_strip.add_css_class("calc-hist-delete");
            let trash = gtk::Image::from_icon_name("user-trash-symbolic");
            trash.set_halign(gtk::Align::End);
            trash.set_valign(gtk::Align::Center);
            trash.set_hexpand(true);
            trash.set_margin_end(24);
            del_strip.append(&trash);

            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&del_strip));
            overlay.add_overlay(&fg_list);

            // Leftward drag on the foreground: fade + nudge for feedback, and
            // commit a delete past the distance threshold. Default (Bubble)
            // phase + a dead-zone means a plain tap never Claims, so the
            // ActionRow's own activation (tap-to-insert) still fires.
            let drag = gtk::GestureDrag::new();
            drag.connect_drag_update(clone!(
                #[weak]
                fg_list,
                #[upgrade_or_default]
                move |g, off_x, off_y| {
                    if off_x < -12.0 && off_x.abs() > off_y.abs() {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        fg_list.set_opacity((1.0 - (off_x.abs() / 200.0)).clamp(0.3, 1.0));
                        fg_list.set_margin_end((off_x.abs() as i32).min(120));
                    }
                }
            ));
            drag.connect_drag_end(clone!(
                #[weak]
                ui,
                #[weak]
                fg_list,
                #[upgrade_or_default]
                move |_g, off_x, off_y| {
                    if off_x < -100.0 && off_x.abs() > off_y.abs() {
                        // Commit delete: mutate + persist, then re-render the page.
                        {
                            let mut h = ui.history.borrow_mut();
                            h.remove(storage_idx);
                            h.save();
                        }
                        ui.nav.pop();
                        show_history(&ui);
                    } else {
                        // Spring back to rest.
                        fg_list.set_opacity(1.0);
                        fg_list.set_margin_end(0);
                    }
                }
            ));
            fg_list.add_controller(drag);

            if let Some(g) = &group {
                g.add(&overlay);
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

/// Recompute the converted value and update the two display labels from the
/// current ConverterState. `top`/`bottom` are the two display labels.
fn converter_refresh(ui: &Ui, top: &gtk::Label, bottom: &gtk::Label) {
    let st = ui.converter.borrow();
    let units = st.category.units();
    let from = &units[st.from_idx.min(units.len() - 1)];
    let to = &units[st.to_idx.min(units.len() - 1)];
    let shown_input = if st.input.is_empty() { "0".to_string() } else { st.input.clone() };
    top.set_text(&format!("{} {}", shown_input, from.symbol));
    let result = crate::convert::convert(st.category, from, to, st.value());
    bottom.set_text(&format!("{} {}", crate::convert::format_conversion(result), to.symbol));
}

/// Build a round converter keypad button of the given label + style class that
/// mutates the converter state then refreshes the two display labels.
fn conv_key(
    label: &str,
    class: &str,
    ui: &Ui,
    top: &gtk::Label,
    bottom: &gtk::Label,
    mutate: impl Fn(&mut ConverterState) + 'static,
) -> gtk::Button {
    let btn = gtk::Button::builder()
        .label(label)
        .css_classes(["calc-btn", class])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    btn.connect_clicked(clone!(
        #[weak]
        ui,
        #[weak]
        top,
        #[weak]
        bottom,
        #[upgrade_or_default]
        move |_| {
            mutate(&mut ui.converter.borrow_mut());
            converter_refresh(&ui, &top, &bottom);
        }
    ));
    btn
}

/// Build and push the unit-converter navigation page (reduced keypad, no OSK).
fn show_converter(ui: &Ui) {
    {
        let cat = settings::converter_category();
        let mut st = ui.converter.borrow_mut();
        st.category = cat;
        st.from_idx = category_index_of(cat, cat.default_from().id);
        st.to_idx = category_index_of(cat, cat.default_to().id);
        st.input.clear();
    }

    let top_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-expression", "calc-secondary"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    let bottom_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-result", "calc-primary"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    bottom_label.set_selectable(false);

    let display = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(20)
        .margin_end(20)
        .margin_top(8)
        .margin_bottom(4)
        .build();
    display.append(&top_label);
    display.append(&bottom_label);

    let all_cats = crate::convert::Category::all();
    let cat_names: Vec<&str> = all_cats.iter().map(|c| c.name()).collect();
    let cat_list = gtk::StringList::new(&cat_names);
    let cat_row = adw::ComboRow::builder()
        .title("Category")
        .model(&cat_list)
        .build();
    let cur_cat = ui.converter.borrow().category;
    let cur_cat_idx = all_cats.iter().position(|c| *c == cur_cat).unwrap_or(0) as u32;
    cat_row.set_selected(cur_cat_idx);

    let unit_list_for = |cat: crate::convert::Category| -> gtk::StringList {
        let labels: Vec<String> = cat
            .units()
            .iter()
            .map(|u| format!("{} ({})", u.name, u.symbol))
            .collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        gtk::StringList::new(&refs)
    };

    let from_row = adw::ComboRow::builder().title("From").build();
    from_row.set_model(Some(&unit_list_for(cur_cat)));
    from_row.set_selected(ui.converter.borrow().from_idx as u32);

    let swap_btn = gtk::Button::builder()
        .icon_name("object-flip-vertical-symbolic")
        .tooltip_text("Swap")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();

    let to_row = adw::ComboRow::builder().title("To").build();
    to_row.set_model(Some(&unit_list_for(cur_cat)));
    to_row.set_selected(ui.converter.borrow().to_idx as u32);
    to_row.add_suffix(&swap_btn);

    let group = adw::PreferencesGroup::builder().build();
    group.add(&cat_row);
    group.add(&from_row);
    group.add(&to_row);

    from_row.connect_selected_notify(clone!(
        #[weak]
        ui,
        #[weak]
        top_label,
        #[weak]
        bottom_label,
        #[upgrade_or_default]
        move |row| {
            ui.converter.borrow_mut().from_idx = row.selected() as usize;
            converter_refresh(&ui, &top_label, &bottom_label);
        }
    ));

    to_row.connect_selected_notify(clone!(
        #[weak]
        ui,
        #[weak]
        top_label,
        #[weak]
        bottom_label,
        #[upgrade_or_default]
        move |row| {
            ui.converter.borrow_mut().to_idx = row.selected() as usize;
            converter_refresh(&ui, &top_label, &bottom_label);
        }
    ));

    cat_row.connect_selected_notify(clone!(
        #[weak]
        ui,
        #[weak]
        from_row,
        #[weak]
        to_row,
        #[weak]
        top_label,
        #[weak]
        bottom_label,
        #[upgrade_or_default]
        move |row| {
            let all = crate::convert::Category::all();
            let new_cat = all[(row.selected() as usize).min(all.len() - 1)];
            let (new_from, new_to) = {
                let mut st = ui.converter.borrow_mut();
                st.category = new_cat;
                st.from_idx = category_index_of(new_cat, new_cat.default_from().id);
                st.to_idx = category_index_of(new_cat, new_cat.default_to().id);
                (st.from_idx, st.to_idx)
            };
            let from_labels: Vec<String> = new_cat
                .units()
                .iter()
                .map(|u| format!("{} ({})", u.name, u.symbol))
                .collect();
            let from_refs: Vec<&str> = from_labels.iter().map(|s| s.as_str()).collect();
            from_row.set_model(Some(&gtk::StringList::new(&from_refs)));
            to_row.set_model(Some(&gtk::StringList::new(&from_refs)));
            from_row.set_selected(new_from as u32);
            to_row.set_selected(new_to as u32);
            settings::set_converter_category(new_cat);
            converter_refresh(&ui, &top_label, &bottom_label);
        }
    ));

    swap_btn.connect_clicked(clone!(
        #[weak]
        ui,
        #[weak]
        from_row,
        #[weak]
        to_row,
        #[weak]
        top_label,
        #[weak]
        bottom_label,
        #[upgrade_or_default]
        move |_| {
            let (f, t) = {
                let mut st = ui.converter.borrow_mut();
                let (f, t) = (st.to_idx, st.from_idx);
                st.from_idx = f;
                st.to_idx = t;
                (f, t)
            };
            from_row.set_selected(f as u32);
            to_row.set_selected(t as u32);
            converter_refresh(&ui, &top_label, &bottom_label);
        }
    ));

    let pad = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(8)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .build();

    let digit = |d: char| conv_key(
        &d.to_string(),
        "calc-digit",
        ui,
        &top_label,
        &bottom_label,
        move |st| {
            if st.input == "0" {
                st.input.clear();
            }
            st.input.push(d);
        },
    );

    pad.attach(&digit('7'), 0, 0, 1, 1);
    pad.attach(&digit('8'), 1, 0, 1, 1);
    pad.attach(&digit('9'), 2, 0, 1, 1);
    pad.attach(
        &conv_key("AC", "calc-clear", ui, &top_label, &bottom_label, |st| st.input.clear()),
        3, 0, 1, 1,
    );

    pad.attach(&digit('4'), 0, 1, 1, 1);
    pad.attach(&digit('5'), 1, 1, 1, 1);
    pad.attach(&digit('6'), 2, 1, 1, 1);

    let back = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .css_classes(["calc-btn", "calc-function"])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    back.connect_clicked(clone!(
        #[weak]
        ui,
        #[weak]
        top_label,
        #[weak]
        bottom_label,
        #[upgrade_or_default]
        move |_| {
            ui.converter.borrow_mut().input.pop();
            converter_refresh(&ui, &top_label, &bottom_label);
        }
    ));
    pad.attach(&back, 3, 1, 1, 1);

    pad.attach(&digit('1'), 0, 2, 1, 1);
    pad.attach(&digit('2'), 1, 2, 1, 1);
    pad.attach(&digit('3'), 2, 2, 1, 1);
    pad.attach(
        &conv_key("\u{00B1}", "calc-function", ui, &top_label, &bottom_label, |st| {
            if st.input.starts_with('-') {
                st.input.remove(0);
            } else if !st.input.is_empty() && st.input != "0" {
                st.input.insert(0, '-');
            }
        }),
        3, 2, 1, 1,
    );

    pad.attach(&digit('0'), 0, 3, 2, 1);
    pad.attach(
        &conv_key(".", "calc-digit", ui, &top_label, &bottom_label, |st| {
            if st.input.is_empty() {
                st.input.push_str("0.");
            } else if !st.input.contains('.') {
                st.input.push('.');
            }
        }),
        2, 3, 1, 1,
    );

    let long = gtk::GestureLongPress::new();
    long.connect_pressed(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_, _, _| {
            if let Some(display) = gdk::Display::default() {
                let st = ui.converter.borrow();
                let units = st.category.units();
                let from = &units[st.from_idx.min(units.len() - 1)];
                let to = &units[st.to_idx.min(units.len() - 1)];
                let r = crate::convert::convert(st.category, from, to, st.value());
                display.clipboard().set_text(&crate::convert::format_conversion(r));
            }
        }
    ));
    bottom_label.add_controller(long);
    let right = gtk::GestureClick::new();
    right.set_button(gdk::BUTTON_SECONDARY);
    right.connect_pressed(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_, _, _, _| {
            if let Some(display) = gdk::Display::default() {
                let st = ui.converter.borrow();
                let units = st.category.units();
                let from = &units[st.from_idx.min(units.len() - 1)];
                let to = &units[st.to_idx.min(units.len() - 1)];
                let r = crate::convert::convert(st.category, from, to, st.value());
                display.clipboard().set_text(&crate::convert::format_conversion(r));
            }
        }
    ));
    bottom_label.add_controller(right);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(16)
        .build();
    content.append(&display);
    content.append(&group);
    content.append(&pad);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();

    let clamp = adw::Clamp::builder()
        .maximum_size(420)
        .child(&scroller)
        .build();

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&clamp));

    let page = adw::NavigationPage::builder()
        .title("Convert")
        .tag("converter")
        .child(&toolbar)
        .build();

    converter_refresh(ui, &top_label, &bottom_label);
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
