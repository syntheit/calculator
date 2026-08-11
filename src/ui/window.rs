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
use crate::engine::format::NumLocale;
use crate::history::{self, History};
use crate::programmer::{Base, Width};
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
    sci_sinh: Rc<Vec<gtk::Button>>,
    sci_cosh: Rc<Vec<gtk::Button>>,
    sci_tanh: Rc<Vec<gtk::Button>>,
    /// The AdwNavigationView the history page is pushed onto.
    nav: adw::NavigationView,
    /// Converter page state (category + selected unit indices + input string).
    /// A plain Rc<RefCell<>> holder, entirely separate from the Calculator
    /// state machine. Reset each time the converter page is opened.
    converter: Rc<RefCell<ConverterState>>,
    /// The mode container: swaps calculator ⇄ converter pages.
    content_stack: adw::ViewStack,
    /// The header center title (retitled on mode switch).
    window_title: adw::WindowTitle,
    /// The stateful "mode" radio action (state set on startup restore).
    mode_action: gio::SimpleAction,
    /// The header History button (calculator-mode only; hidden elsewhere).
    history_btn: gtk::Button,
    /// The converter's top (input echo) display label — shared so a locale
    /// change can refresh the converter without rebuilding its page.
    conv_top_label: gtk::Label,
    /// The converter's bottom (result) display label — shared, see above.
    conv_bottom_label: gtk::Label,
    /// Programmer-mode state machine (base/width/signed + expression buffer).
    prog: Rc<RefCell<crate::prog_state::ProgState>>,
    /// The 4 base-row flat buttons, in fixed order [Hex, Dec, Oct, Bin].
    prog_rows: Rc<Vec<gtk::Button>>,
    /// The right-hand value label of each base row, same [Hex,Dec,Oct,Bin] order.
    prog_row_values: Rc<Vec<gtk::Label>>,
    /// The A–F hex-digit keypad buttons (A,B,C,D,E,F), toggled by base.
    prog_hex_btns: Rc<Vec<gtk::Button>>,
    /// The 0–9 digit keypad buttons, index == digit value, toggled by base.
    prog_digit_btns: Rc<Vec<gtk::Button>>,
    /// The 4 linked width toggle buttons [W8,W16,W32,W64].
    prog_width_btns: Rc<Vec<gtk::ToggleButton>>,
    /// The signed/unsigned toggle (active == signed).
    prog_signed_btn: gtk::ToggleButton,
    /// The programmer-mode expression / error line.
    prog_expr_label: gtk::Label,
    /// Financial-mode state (selected calculator + per-field input strings).
    fin: Rc<RefCell<crate::fin_state::FinState>>,
    /// The financial calculator picker.
    fin_calc_row: adw::ComboRow,
    /// The PreferencesGroup holding the per-field input rows (rebuilt on calc change).
    fin_field_group: adw::PreferencesGroup,
    /// The flat, selectable per-field rows (rebuilt on calc change), field order.
    fin_field_rows: Rc<RefCell<Vec<gtk::Button>>>,
    /// The per-field VALUE labels, same index order, for the render pass.
    fin_field_values: Rc<RefCell<Vec<gtk::Label>>>,
    /// The financial result row's value label.
    fin_result_label: gtk::Label,
    /// The financial result row (its title tracks the selected calculator).
    fin_result_row: adw::ActionRow,
    /// The last calculator-family mode ("calculator" or "programmer") we were
    /// in before entering the converter, so the Convert toggle can return there.
    last_calc_mode: Rc<RefCell<String>>,
    /// The header Convert toggle button (active iff the converter page shows).
    convert_btn: gtk::ToggleButton,
}

impl Ui {
    /// The user's chosen number-format locale.
    fn locale(&self) -> NumLocale {
        settings::number_format()
    }

    /// Redraw the display from the calculator state. Called after every input.
    fn render(&self) {
        let calc = self.calc.borrow();
        let loc = self.locale();

        // Reset transient classes; re-added below as the state demands.
        for w in [&self.expr_label, &self.result_label] {
            w.remove_css_class("calc-error");
            w.remove_css_class("calc-primary");
            w.remove_css_class("calc-secondary");
        }

        match calc.state() {
            CalcState::Error => {
                // Keep the (offending) expression up top, show the message big.
                self.expr_label.set_text(&calc.display_expression_with(loc.group(), loc.decimal()));
                self.expr_label.add_css_class("calc-secondary");
                self.expr_label.add_css_class("calc-error");
                let msg = calc.error_message().unwrap_or_default();
                self.result_label.set_text(&msg);
                self.result_label.add_css_class("calc-primary");
                self.result_label.add_css_class("calc-error");
            }
            CalcState::Result => {
                // Swap emphasis: expression dims above, result is the big line.
                self.expr_label.set_text(&calc.display_expression_with(loc.group(), loc.decimal()));
                self.expr_label.add_css_class("calc-secondary");
                match calc.current_value() {
                    Some(v) => {
                        self.result_label.set_text(&crate::engine::format::format_result_locale(v, loc));
                        self.result_label.add_css_class("calc-primary");
                    }
                    None => self.result_label.set_text(""),
                }
            }
            CalcState::Input => {
                self.expr_label.set_text(&calc.display_expression_with(loc.group(), loc.decimal()));
                match calc.live_value() {
                    Some(v) => {
                        self.result_label.set_text(&crate::engine::format::format_result_locale(v, loc));
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
        for b in self.sci_sinh.iter() {
            b.set_label(if inv { "asinh" } else { "sinh" });
        }
        for b in self.sci_cosh.iter() {
            b.set_label(if inv { "acosh" } else { "cosh" });
        }
        for b in self.sci_tanh.iter() {
            b.set_label(if inv { "atanh" } else { "tanh" });
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
        let text = match self.content_stack.visible_child_name().as_deref() {
            Some("converter") => {
                let st = self.converter.borrow();
                let units = st.category.units();
                let from = &units[st.from_idx.min(units.len() - 1)];
                let to = &units[st.to_idx.min(units.len() - 1)];
                let r = crate::convert::convert(st.category, from, to, st.value());
                if crate::convert::is_overflow(r) {
                    String::new()
                } else {
                    crate::engine::format::format_result_locale(r, self.locale())
                }
            }
            Some("programmer") => {
                let st = self.prog.borrow();
                if st.error_preview().is_some() {
                    String::new()
                } else {
                    let b = st.base();
                    st.display(b)
                }
            }
            Some("financial") => self.fin_result_label.text().to_string(),
            _ => {
                let calc = self.calc.borrow();
                if !self.result_label.text().is_empty() {
                    self.result_label.text().to_string()
                } else if let Some(v) = calc.current_value() {
                    crate::engine::format::format_result_locale(v, self.locale())
                } else {
                    calc.display_expression_with(self.locale().group(), self.locale().decimal())
                }
            }
        };
        if text.is_empty() {
            return;
        }
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
    }

    /// Redraw the programmer-mode page from ProgState. Borrow-safe: no RefCell
    /// borrow is held across any widget setter.
    fn render_prog(&self) {
        // Read everything needed into locals, then drop the borrow.
        let (latched_err, err_msg, live_err, active_base, hex, dec, oct, bin, expr) = {
            let st = self.prog.borrow();
            let latched = st.error().map(|s| s.to_string());
            let live = st.error_preview();
            (
                latched.is_some(),
                latched.clone().or_else(|| live.clone()).unwrap_or_default(),
                live.is_some(),
                st.base(),
                st.display(Base::Hex),
                st.display(Base::Dec),
                st.display(Base::Oct),
                st.display(Base::Bin),
                st.expression(),
            )
        };
        let has_err = latched_err || live_err;

        // Grouping for readability.
        let loc = self.locale();
        let g = loc.group();
        let hex_g = group_from_right(&hex, 4, ' ');
        let oct_g = group_from_right(&oct, 3, ' ');
        let bin_g = group_from_right(&bin, 4, ' ');
        // Decimal: strip a leading '-', group the absolute digits, re-prepend.
        let dec_g = if let Some(rest) = dec.strip_prefix('-') {
            format!("-{}", group_from_right(rest, 3, g))
        } else {
            group_from_right(&dec, 3, g)
        };

        // Set the 4 value labels [Hex,Dec,Oct,Bin].
        let grouped = [hex_g, dec_g, oct_g, bin_g];
        for (label, text) in self.prog_row_values.iter().zip(grouped.iter()) {
            // On a live arithmetic error, `display()` returns "0"; blank the
            // rows instead of showing a bogus value.
            if live_err {
                label.set_text("");
            } else {
                label.set_text(text);
            }
        }

        // Highlight the active base row (0=Hex,1=Dec,2=Oct,3=Bin).
        let active_idx = match active_base {
            Base::Hex => 0,
            Base::Dec => 1,
            Base::Oct => 2,
            Base::Bin => 3,
        };
        for (i, row) in self.prog_rows.iter().enumerate() {
            if i == active_idx {
                row.add_css_class("calc-primary");
            } else {
                row.remove_css_class("calc-primary");
            }
        }

        // Error state: recolor value labels + expr line, set expr text.
        for label in self.prog_row_values.iter() {
            if has_err {
                label.add_css_class("calc-error");
            } else {
                label.remove_css_class("calc-error");
            }
        }
        if has_err {
            self.prog_expr_label.add_css_class("calc-error");
            self.prog_expr_label.set_text(&err_msg);
        } else {
            self.prog_expr_label.remove_css_class("calc-error");
            self.prog_expr_label.set_text(&expr);
        }

        // Sync width toggles (guarded so we don't re-fire `toggled`).
        let want_w = { self.prog.borrow().width() };
        let want_idx = match want_w {
            Width::W8 => 0,
            Width::W16 => 1,
            Width::W32 => 2,
            Width::W64 => 3,
        };
        for (i, btn) in self.prog_width_btns.iter().enumerate() {
            let want = i == want_idx;
            if btn.is_active() != want {
                btn.set_active(want);
            }
        }

        // Sync signed toggle label + state (guarded).
        let signed = { self.prog.borrow().signed() };
        self.prog_signed_btn
            .set_label(if signed { "signed" } else { "unsigned" });
        if self.prog_signed_btn.is_active() != signed {
            self.prog_signed_btn.set_active(signed);
        }
    }

    /// Enable each digit key only when it's a valid digit for the active base:
    /// BIN→0,1; OCT→0-7; DEC→0-9; HEX→0-9 + A-F.
    fn prog_sync_digit_sensitivity(&self) {
        let base = { self.prog.borrow().base() };
        let hex_chars = ['A', 'B', 'C', 'D', 'E', 'F'];
        for (btn, c) in self.prog_hex_btns.iter().zip(hex_chars.iter()) {
            let ok = base.is_valid_digit(*c);
            btn.set_sensitive(ok);
            if ok {
                btn.remove_css_class("calc-disabled");
            } else {
                btn.add_css_class("calc-disabled");
            }
        }
        for (i, btn) in self.prog_digit_btns.iter().enumerate() {
            let c = std::char::from_digit(i as u32, 10).unwrap();
            let ok = base.is_valid_digit(c);
            btn.set_sensitive(ok);
            if ok {
                btn.remove_css_class("calc-disabled");
            } else {
                btn.add_css_class("calc-disabled");
            }
        }
    }

    /// Redraw the financial-mode page from FinState. Borrow-safe: no RefCell
    /// borrow is held across any widget setter.
    fn render_fin(&self) {
        // Read everything needed into locals, then drop the borrow.
        let (values, active, result, result_title) = {
            let st = self.fin.borrow();
            let n = st.selected().fields().len();
            let values: Vec<String> = (0..n).map(|i| st.field_value(i).to_string()).collect();
            (values, st.active(), st.compute(), st.selected().result_label())
        };
        self.fin_result_row.set_title(result_title);

        // Value labels: show the raw string, or a dim "0" when empty.
        let labels = self.fin_field_values.borrow();
        for (i, label) in labels.iter().enumerate() {
            let text = match values.get(i) {
                Some(s) if !s.is_empty() => s.clone(),
                _ => "0".to_string(),
            };
            label.set_text(&text);
        }
        drop(labels);

        // Highlight the active field row with `.calc-primary`.
        let rows = self.fin_field_rows.borrow();
        for (i, row) in rows.iter().enumerate() {
            if i == active {
                row.add_css_class("calc-primary");
            } else {
                row.remove_css_class("calc-primary");
            }
        }
        drop(rows);

        // Result: None → incomplete (neutral blank); Some(Ok) → formatted;
        // Some(Err) → error message + error class.
        match result {
            None => {
                self.fin_result_label.remove_css_class("calc-error");
                self.fin_result_label.set_text("");
            }
            Some(Ok(v)) => {
                self.fin_result_label.remove_css_class("calc-error");
                self.fin_result_label
                    .set_text(&crate::engine::format::format_result_locale(v, self.locale()));
            }
            Some(Err(e)) => {
                self.fin_result_label.add_css_class("calc-error");
                self.fin_result_label.set_text(&e.to_string());
            }
        }
    }

    /// Rebuild the per-field input rows for the currently selected calculator.
    /// Called on startup and whenever the calculator picker changes.
    fn fin_rebuild_fields(&self) {
        // Read the selected calc's fields (they're &'static) with a short borrow.
        let fields: &'static [crate::fin_state::FinField] = {
            let st = self.fin.borrow();
            st.selected().fields()
        };

        // Remove the old rows from the group.
        {
            let old = self.fin_field_rows.borrow();
            for row in old.iter() {
                self.fin_field_group.remove(row);
            }
        }

        // Build fresh rows + value labels.
        let mut new_rows: Vec<gtk::Button> = Vec::with_capacity(fields.len());
        let mut new_values: Vec<gtk::Label> = Vec::with_capacity(fields.len());
        for (idx, field) in fields.iter().enumerate() {
            let name_label = gtk::Label::builder()
                .label(field.label)
                .css_classes(["calc-fin-label"])
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            let value_label = gtk::Label::builder()
                .label("0")
                .css_classes(["calc-fin-value"])
                .halign(gtk::Align::End)
                .hexpand(true)
                .xalign(1.0)
                .single_line_mode(true)
                .ellipsize(gtk::pango::EllipsizeMode::Start)
                .build();
            let suffix_label = gtk::Label::builder()
                .label(field.suffix)
                .css_classes(["calc-fin-suffix"])
                .halign(gtk::Align::End)
                .build();
            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            row_box.append(&name_label);
            row_box.append(&value_label);
            row_box.append(&suffix_label);
            let row_btn = gtk::Button::builder()
                .css_classes(["calc-fin-row", "flat"])
                .can_focus(false)
                .hexpand(true)
                .child(&row_box)
                .build();
            row_btn.connect_clicked(clone!(
                #[weak(rename_to = ui)]
                self,
                #[upgrade_or_default]
                move |_| {
                    {
                        let mut st = ui.fin.borrow_mut();
                        st.set_active(idx);
                    }
                    ui.render_fin();
                }
            ));
            self.fin_field_group.add(&row_btn);
            new_rows.push(row_btn);
            new_values.push(value_label);
        }

        *self.fin_field_rows.borrow_mut() = new_rows;
        *self.fin_field_values.borrow_mut() = new_values;
        self.render_fin();
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

    let content_stack = adw::ViewStack::new();
    content_stack.set_hhomogeneous(false); // CRITICAL anti-regression
    content_stack.set_vhomogeneous(false); // CRITICAL anti-regression
    let window_title = adw::WindowTitle::new("Calculator", "");
    let history_btn = gtk::Button::builder()
        .icon_name("document-open-recent-symbolic")
        .tooltip_text("History")
        .build();
    let convert_btn = gtk::ToggleButton::builder()
        .icon_name("object-flip-vertical-symbolic")
        .tooltip_text("Unit converter")
        .css_classes(["flat"])
        .build();
    let mode_action = gio::SimpleAction::new_stateful(
        "mode",
        Some(glib::VariantTy::STRING),
        &"calculator".to_variant(),
    );
    let conv_top_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-expression", "calc-secondary"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    let conv_bottom_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-result", "calc-primary"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    conv_bottom_label.set_selectable(false);

    // ── Programmer-mode widgets (built before `ui` so its fields are live) ──
    let prog = Rc::new(RefCell::new(crate::prog_state::ProgState::new(
        settings::prog_base(),
        settings::prog_width(),
        settings::prog_signed(),
    )));
    // 4 base rows [Hex,Dec,Oct,Bin]: a flat button whose child holds a name +
    // value label. Value labels are kept for the render pass.
    let mut prog_rows_v: Vec<gtk::Button> = Vec::with_capacity(4);
    let mut prog_row_values_v: Vec<gtk::Label> = Vec::with_capacity(4);
    for (i, (_base, name)) in [
        (Base::Hex, "HEX"),
        (Base::Dec, "DEC"),
        (Base::Oct, "OCT"),
        (Base::Bin, "BIN"),
    ]
    .iter()
    .enumerate()
    {
        let name_label = gtk::Label::builder()
            .label(*name)
            .css_classes(["calc-prog-baselabel"])
            .width_request(46)
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .build();
        let value_label = gtk::Label::builder()
            .label("0")
            .css_classes(["calc-prog-value"])
            .halign(gtk::Align::End)
            .hexpand(true)
            .xalign(1.0)
            .build();
        if i == 3 {
            // BIN row: allow up to 2 lines so 64-bit binary wraps within the
            // fixed-height block rather than reflowing it.
            value_label.set_single_line_mode(false);
            value_label.set_wrap(true);
            value_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value_label.set_lines(2);
            value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        } else {
            value_label.set_single_line_mode(true);
            value_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        }
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        row_box.append(&name_label);
        row_box.append(&value_label);
        let row_btn = gtk::Button::builder()
            .css_classes(["calc-prog-row", "flat"])
            .can_focus(false)
            .hexpand(true)
            .child(&row_box)
            .build();
        prog_rows_v.push(row_btn);
        prog_row_values_v.push(value_label);
    }
    // A–F hex keypad buttons (built here, wired + placed in the page builder).
    let mut prog_hex_btns_v: Vec<gtk::Button> = Vec::with_capacity(6);
    for label in ["A", "B", "C", "D", "E", "F"] {
        prog_hex_btns_v.push(
            gtk::Button::builder()
                .label(label)
                .css_classes(["calc-btn", "calc-btn-prog", "calc-digit"])
                .hexpand(true)
                .vexpand(false)
                .can_focus(false)
                .build(),
        );
    }
    // 0–9 digit keypad buttons (built here, wired + placed in the page builder).
    let mut prog_digit_btns_v: Vec<gtk::Button> = Vec::with_capacity(10);
    for d in 0u32..=9 {
        prog_digit_btns_v.push(
            gtk::Button::builder()
                .label(d.to_string())
                .css_classes(["calc-btn", "calc-btn-prog", "calc-digit"])
                .hexpand(true)
                .vexpand(false)
                .can_focus(false)
                .build(),
        );
    }
    // 4 width toggles [W8,W16,W32,W64], grouped so exactly one is active.
    let mut prog_width_btns_v: Vec<gtk::ToggleButton> = Vec::with_capacity(4);
    for label in ["8", "16", "32", "64"] {
        prog_width_btns_v.push(
            gtk::ToggleButton::builder()
                .label(label)
                .can_focus(false)
                .build(),
        );
    }
    let prog_signed_btn = gtk::ToggleButton::builder()
        .label("signed")
        .css_classes(["calc-prog-sign"])
        .can_focus(false)
        .build();
    let prog_expr_label = gtk::Label::builder()
        .label("")
        .css_classes(["calc-expression", "calc-secondary"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();

    // ── Financial-mode widgets (built before `ui` so its fields are live) ──
    let fin = Rc::new(RefCell::new(crate::fin_state::FinState::new(
        settings::fin_calc(),
    )));
    let fin_calc_names: Vec<&str> = crate::fin_state::FinCalc::all()
        .iter()
        .map(|c| c.title())
        .collect();
    let fin_calc_model = gtk::StringList::new(&fin_calc_names);
    let fin_calc_row = adw::ComboRow::builder()
        .title("Calculator")
        .model(&fin_calc_model)
        .build();
    let fin_field_group = adw::PreferencesGroup::builder().build();
    let fin_result_label = gtk::Label::builder()
        .label("0")
        .css_classes(["calc-fin-result"])
        .halign(gtk::Align::End)
        .xalign(1.0)
        .wrap(false)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    let fin_result_row = adw::ActionRow::builder()
        .title(settings::fin_calc().result_label())
        .build();
    fin_result_row.add_suffix(&fin_result_label);

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
        sci_sinh: Rc::new(vec![sp.sinh.clone(), sl.sinh.clone()]),
        sci_cosh: Rc::new(vec![sp.cosh.clone(), sl.cosh.clone()]),
        sci_tanh: Rc::new(vec![sp.tanh.clone(), sl.tanh.clone()]),
        nav: nav.clone(),
        converter: Rc::new(RefCell::new(ConverterState {
            category: start_cat,
            from_idx: category_index_of(start_cat, start_cat.default_from().id),
            to_idx: category_index_of(start_cat, start_cat.default_to().id),
            input: String::new(),
        })),
        content_stack: content_stack.clone(),
        window_title: window_title.clone(),
        mode_action: mode_action.clone(),
        history_btn: history_btn.clone(),
        conv_top_label: conv_top_label.clone(),
        conv_bottom_label: conv_bottom_label.clone(),
        prog: prog.clone(),
        prog_rows: Rc::new(prog_rows_v.clone()),
        prog_row_values: Rc::new(prog_row_values_v.clone()),
        prog_hex_btns: Rc::new(prog_hex_btns_v.clone()),
        prog_digit_btns: Rc::new(prog_digit_btns_v.clone()),
        prog_width_btns: Rc::new(prog_width_btns_v.clone()),
        prog_signed_btn: prog_signed_btn.clone(),
        prog_expr_label: prog_expr_label.clone(),
        fin: fin.clone(),
        fin_calc_row: fin_calc_row.clone(),
        fin_field_group: fin_field_group.clone(),
        fin_field_rows: Rc::new(RefCell::new(Vec::new())),
        fin_field_values: Rc::new(RefCell::new(Vec::new())),
        fin_result_label: fin_result_label.clone(),
        fin_result_row: fin_result_row.clone(),
        last_calc_mode: Rc::new(RefCell::new(String::from("calculator"))),
        convert_btn: convert_btn.clone(),
    };

    // Wire both stateful button sets (each set wired exactly once — no widget
    // is double-connected).
    wire_sci_buttons(&ui, &sp);
    wire_sci_buttons(&ui, &sl);

    ui.mode_action.connect_activate(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |action, param| {
            let Some(target) = param.and_then(|p| p.str().map(|s| s.to_string())) else {
                return;
            };
            action.set_state(&target.to_variant());
            switch_mode(&ui, &target);
        }
    ));

    // Apply the persisted inverse mode BEFORE the first sync/render, using the
    // top-of-fn `calc` local (no active borrow conflict here).
    calc.borrow_mut().set_inv(settings::inverse_mode());

    // ── Header: history (left), kebab (right) ────────────────────────────
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&window_title));
    history_btn.connect_clicked(clone!(
        #[weak]
        ui,
        move |_| show_history(&ui)
    ));
    header.pack_start(&history_btn);
    header.pack_start(&convert_btn);

    // The header Convert toggle drives the "converter" mode directly (it is not
    // part of the hamburger radio menu). It is a TOGGLE with a re-entrancy guard:
    // switch_mode also syncs this button's active state, which re-fires
    // `toggled`, so we only act when the button's state and the visible page
    // actually disagree.
    convert_btn.connect_toggled(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |btn| {
            let in_converter =
                ui.content_stack.visible_child_name().as_deref() == Some("converter");
            if btn.is_active() == in_converter {
                return; // already in sync — this toggle was programmatic
            }
            if btn.is_active() {
                // Entering converter: remember where we came from, then switch.
                let current = ui
                    .content_stack
                    .visible_child_name()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "calculator".to_string());
                if current != "converter" {
                    *ui.last_calc_mode.borrow_mut() = current;
                }
                ui.mode_action.set_state(&"converter".to_variant());
                switch_mode(&ui, "converter");
            } else {
                // Leaving converter: return to the last calculator-family mode.
                let back = ui.last_calc_mode.borrow().clone();
                ui.mode_action.set_state(&back.to_variant());
                switch_mode(&ui, &back);
            }
        }
    ));

    // Kebab menu (Mode / Copy / Clear history / Preferences / About), backed by
    // a sectioned gio::Menu model.
    let menu_model = gio::Menu::new();

    let mode_section = gio::Menu::new();
    mode_section.append(Some("Calculator"), Some("calc.mode::calculator"));
    mode_section.append(Some("Programmer"), Some("calc.mode::programmer"));
    mode_section.append(Some("Financial"), Some("calc.mode::financial"));
    // Convert lives in a dedicated header toggle button, not this radio menu.
    menu_model.append_section(Some("Mode"), &mode_section);

    let ops_section = gio::Menu::new();
    ops_section.append(Some("Copy result"), Some("calc.copy"));
    ops_section.append(Some("Clear history"), Some("calc.clear-history"));
    menu_model.append_section(None, &ops_section);

    let app_section = gio::Menu::new();
    app_section.append(Some("Preferences"), Some("calc.preferences"));
    app_section.append(Some("About Calculator"), Some("calc.about"));
    menu_model.append_section(None, &app_section);

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
    // The calculator mode's page: display (top) + keypad (bottom).
    let calculator_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    calculator_page.append(&display);
    calculator_page.append(&keypad_clamp);

    content_stack.add_titled(&calculator_page, Some("calculator"), "Calculator");
    let converter_page = build_converter_page(&ui);
    content_stack.add_titled(&converter_page, Some("converter"), "Convert");
    let programmer_page = build_programmer_page(
        &ui,
        prog_rows_v,
        prog_hex_btns_v,
        prog_digit_btns_v,
        prog_width_btns_v,
        &prog_signed_btn,
        &prog_expr_label,
    );
    content_stack.add_titled(&programmer_page, Some("programmer"), "Programmer");
    let financial_page = build_financial_page(&ui, &fin_calc_row, &fin_field_group, &fin_result_row);
    content_stack.add_titled(&financial_page, Some("financial"), "Financial");

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content_stack));

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
    bp_landscape.add_setter(&display, "css-classes", Some(&["calc-display", "landscape"].to_value()));
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
    ui.prog_sync_digit_sensitivity();
    ui.render_prog();
    ui.render_fin();
    // Restore the saved top-level mode (stack child, title, history-btn
    // visibility, radio state, and a re-render of the active page).
    // Seed the Convert-toggle return target from persistence FIRST, so if the
    // restored mode is "converter", toggling Convert off returns to the calc
    // family mode we were last in (switch_mode only overwrites this when
    // entering "calculator"/"programmer", never "converter").
    *ui.last_calc_mode.borrow_mut() = settings::last_calc_mode();
    let saved_mode = settings::active_mode();
    let saved_mode = match saved_mode.as_str() {
        "converter" => "converter",
        "programmer" => "programmer",
        "financial" => "financial",
        _ => "calculator",
    };
    ui.mode_action.set_state(&saved_mode.to_variant());
    switch_mode(&ui, saved_mode);
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
    sinh: gtk::Button,
    cosh: gtk::Button,
    tanh: gtk::Button,
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
        sinh: sci_button("sinh"),
        cosh: sci_button("cosh"),
        tanh: sci_button("tanh"),
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
    wire_sci(&s.sinh, ui, |c| c.press_func(Func::Sinh));
    wire_sci(&s.cosh, ui, |c| c.press_func(Func::Cosh));
    wire_sci(&s.tanh, ui, |c| c.press_func(Func::Tanh));
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
    let abs_btn = sci_button("|x|");
    wire_sci(&abs_btn, ui, |c| c.press_abs());
    let log2_btn = sci_button("log\u{2082}");
    wire_sci(&log2_btn, ui, |c| c.press_log2());
    let recip = sci_button("1/x");
    wire_sci(&recip, ui, |c| c.press_reciprocal());
    let negate = sci_button("\u{00B1}");
    wire_sci(&negate, ui, |c| c.press_negate());

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

    // Row 3: sinh cosh tanh log₂
    grid.attach(&s.sinh, 0, 3, 1, 1);
    grid.attach(&s.cosh, 1, 3, 1, 1);
    grid.attach(&s.tanh, 2, 3, 1, 1);
    grid.attach(&log2_btn, 3, 3, 1, 1);

    // Row 4: 1/x |x| ± (cell 3,4 intentionally left empty)
    grid.attach(&recip, 0, 4, 1, 1);
    grid.attach(&abs_btn, 1, 4, 1, 1);
    grid.attach(&negate, 2, 4, 1, 1);

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
    let abs_btn = sci_button("|x|");
    wire_sci(&abs_btn, ui, |c| c.press_abs());
    abs_btn.add_css_class("calc-sci-land");
    abs_btn.set_vexpand(true);
    let log2_btn = sci_button("log\u{2082}");
    wire_sci(&log2_btn, ui, |c| c.press_log2());
    log2_btn.add_css_class("calc-sci-land");
    log2_btn.set_vexpand(true);
    let recip = sci_button("1/x");
    wire_sci(&recip, ui, |c| c.press_reciprocal());
    recip.add_css_class("calc-sci-land");
    recip.set_vexpand(true);
    let negate = sci_button("\u{00B1}");
    wire_sci(&negate, ui, |c| c.press_negate());
    negate.add_css_class("calc-sci-land");
    negate.set_vexpand(true);

    // Landscape sci buttons: shrink class + fill row height.
    for b in [&s.inv, &s.deg, &s.sqrt, &s.sin, &s.ln, &s.cos, &s.log, &s.tan, &s.sinh, &s.cosh, &s.tanh] {
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

    // Row 4: sinh cosh tanh
    grid.attach(&s.sinh, 0, 4, 1, 1);
    grid.attach(&s.cosh, 1, 4, 1, 1);
    grid.attach(&s.tanh, 2, 4, 1, 1);

    // Row 5: log₂ |x| 1/x
    grid.attach(&log2_btn, 0, 5, 1, 1);
    grid.attach(&abs_btn, 1, 5, 1, 1);
    grid.attach(&recip, 2, 5, 1, 1);

    // Row 6: ± (cells 1,6 and 2,6 intentionally left empty)
    grid.attach(&negate, 0, 6, 1, 1);

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

    group.add_action(&ui.mode_action);

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

    let preferences = gio::SimpleAction::new("preferences", None);
    preferences.connect_activate(clone!(
        #[weak]
        ui,
        #[weak]
        window,
        #[upgrade_or_default]
        move |_, _| present_preferences(&ui, &window)
    ));
    group.add_action(&preferences);

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

/// Switch the visible mode: set the stack child, retitle the header, toggle
/// the History button, persist the choice, and re-render the newly-visible
/// mode's display. Borrow-safe: no RefCell borrow is held across
/// set_visible_child_name / render.
fn switch_mode(ui: &Ui, mode: &str) {
    ui.content_stack.set_visible_child_name(mode);
    let title = match mode {
        "converter" => "Convert",
        "programmer" => "Programmer",
        "financial" => "Financial",
        _ => "Calculator",
    };
    ui.window_title.set_title(title);
    // History is a calculator-mode concept; hide the button elsewhere.
    ui.history_btn.set_visible(mode == "calculator");
    // Keep the header Convert toggle in sync no matter which path switched
    // the mode (hamburger radio, startup restore, or the toggle itself).
    // Guard against re-entrancy: only mutate if the state actually differs,
    // so we don't recurse through the `toggled` handler. Since
    // set_visible_child_name(mode) already ran above, the toggled handler's
    // guard sees agreement and early-returns.
    let want_active = mode == "converter";
    if ui.convert_btn.is_active() != want_active {
        ui.convert_btn.set_active(want_active);
    }
    // When we switch INTO a calculator-family mode, remember it as the
    // return target for the Convert toggle — in memory and persisted so the
    // toggle restores the right mode across a restart.
    if mode != "converter" {
        *ui.last_calc_mode.borrow_mut() = mode.to_string();
        settings::set_last_calc_mode(mode);
    }
    settings::set_active_mode(mode);
    match mode {
        "converter" => {
            let top = ui.conv_top_label.clone();
            let bottom = ui.conv_bottom_label.clone();
            converter_refresh(ui, &top, &bottom);
        }
        "programmer" => ui.render_prog(),
        "financial" => ui.render_fin(),
        _ => ui.render(),
    }
}

/// The Preferences dialog: number-format locale selector.
fn present_preferences(ui: &Ui, window: &adw::ApplicationWindow) {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder().title("Formatting").build();

    let model = gtk::StringList::new(&["1,234.56 (English)", "1.234,56 (Spanish)"]);
    let combo = adw::ComboRow::builder()
        .title("Number format")
        .model(&model)
        .build();
    let cur = settings::number_format();
    combo.set_selected(match cur {
        NumLocale::EnUs => 0,
        NumLocale::EsAr => 1,
    });

    combo.connect_selected_notify(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |row| {
            let locale = if row.selected() == 1 {
                NumLocale::EsAr
            } else {
                NumLocale::EnUs
            };
            settings::set_number_format(locale);
            match settings::active_mode().as_str() {
                "converter" => {
                    let top = ui.conv_top_label.clone();
                    let bottom = ui.conv_bottom_label.clone();
                    converter_refresh(&ui, &top, &bottom);
                }
                "programmer" => ui.render_prog(),
                "financial" => ui.render_fin(),
                _ => ui.render(),
            }
        }
    ));

    group.add(&combo);
    page.add(&group);

    let dialog = adw::PreferencesDialog::new();
    dialog.add(&page);
    dialog.present(Some(window));
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

    let clear_btn = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Clear history")
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

        for entry in entries.iter() {
            let label = history::day_label(entry.timestamp, now);
            if current_label.as_deref() != Some(label.as_str()) {
                let g = adw::PreferencesGroup::builder().title(&label).build();
                content.append(&g);
                group = Some(g);
                current_label = Some(label);
            }

            // Plain two-line row added directly to the day-group.
            let row = adw::ActionRow::builder().activatable(true).build();
            row.set_hexpand(true);

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

            // Trailing flat delete button: delete in place (no rebuild).
            let del_btn = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text("Delete entry")
                .css_classes(["flat", "circular"])
                .build();
            let entry_btn = entry.clone();
            let group_btn_ref = group.clone().expect("day-group exists");
            del_btn.connect_clicked(clone!(
                #[weak]
                ui,
                #[weak]
                row,
                #[weak]
                group_btn_ref,
                #[weak]
                content,
                #[weak]
                clear_btn,
                #[upgrade_or_default]
                move |_| {
                    delete_in_place(&ui, &entry_btn, &row, &group_btn_ref, &content, &clear_btn);
                }
            ));
            row.add_suffix(&del_btn);

            // Tap → insert the result into the current expression, pop back.
            let result_value = entry.result.clone();
            row.connect_activated(clone!(
                #[weak]
                ui,
                #[upgrade_or_default]
                move |_| {
                    ui.calc.borrow_mut().insert_result(&result_value);
                    ui.render();
                    ui.nav.pop();
                }
            ));

            // Flick-to-delete: a leftward, horizontal-dominant swipe deletes the
            // row. Default (bubble) phase so tap-to-insert and vertical scroll
            // still win for non-flick gestures.
            let entry_swipe = entry.clone();
            let group_swipe_ref = group.clone().expect("day-group exists");
            let swipe = gtk::GestureSwipe::new();
            swipe.connect_swipe(clone!(
                #[weak]
                ui,
                #[weak]
                row,
                #[weak]
                group_swipe_ref,
                #[weak]
                content,
                #[weak]
                clear_btn,
                #[upgrade_or_default]
                move |_g, velocity_x, velocity_y| {
                    // Leftward, horizontal-dominant flick with enough speed → delete.
                    // NOTE: the velocity threshold (-400.0) is on-device tunable.
                    if velocity_x < -400.0 && velocity_x.abs() > velocity_y.abs() {
                        delete_in_place(&ui, &entry_swipe, &row, &group_swipe_ref, &content, &clear_btn);
                    }
                }
            ));
            row.add_controller(swipe);

            if let Some(g) = &group {
                g.add(&row);
            }
        }
    }
    drop(hist);
    if ui.history.borrow().is_empty() {
        clear_btn.set_sensitive(false);
    }

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

/// Delete `entry` from history by identity, persist, then remove ONLY this
/// row widget from its day-group in place — no nav.pop, no rebuild. If the
/// group is now empty hide it; if all history is gone, show the empty state
/// and disable the Clear-history button. Deleting by identity (value match,
/// not index) stays correct regardless of removal order.
fn delete_in_place(
    ui: &Ui,
    entry: &history::HistoryEntry,
    row: &adw::ActionRow,
    group: &adw::PreferencesGroup,
    content: &gtk::Box,
    clear_btn: &gtk::Button,
) {
    // Mutate + persist history with the borrow scoped and dropped BEFORE any
    // widget mutation.
    {
        let mut h = ui.history.borrow_mut();
        h.remove_entry(entry);
        h.save();
    }
    // Remove only this row widget from its group.
    group.remove(row);
    // If the group has no rows left, hide it (drops its day header too).
    if preferences_group_is_empty(group) {
        group.set_visible(false);
    }
    // If history is now entirely empty, swap in the empty state + disable Clear.
    if ui.history.borrow().is_empty() {
        while let Some(c) = content.first_child() {
            content.remove(&c);
        }
        let empty = adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .title("No history yet")
            .description("Completed calculations will appear here.")
            .vexpand(true)
            .build();
        content.append(&empty);
        clear_btn.set_sensitive(false);
    }
}

/// Whether an AdwPreferencesGroup has no list rows left (used to hide the
/// group + its header once its last entry is deleted).
fn preferences_group_is_empty(group: &adw::PreferencesGroup) -> bool {
    fn count_rows(w: &gtk::Widget) -> usize {
        let mut n = 0;
        if w.is::<gtk::ListBoxRow>() {
            n += 1;
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            n += count_rows(&ch);
            c = ch.next_sibling();
        }
        n
    }
    let mut total = 0;
    let mut cur = group.upcast_ref::<gtk::Widget>().first_child();
    while let Some(w) = cur {
        total += count_rows(&w);
        cur = w.next_sibling();
    }
    total == 0
}

/// Localize a canonical decimal number string (e.g. "1234.5") for display:
/// group the integer digits and swap '.' for the locale decimal separator.
/// A leading '-' and a trailing '.' (mid-entry) are preserved. Does not mutate
/// the canonical string the converter stores internally.
fn localize_decimal_string(s: &str, loc: NumLocale) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    let (sign, body) = if let Some(rest) = s.strip_prefix('-') {
        ("-", rest)
    } else {
        ("", s)
    };
    match body.split_once('.') {
        Some((int_part, frac)) => {
            let grouped = group_from_right(int_part, 3, loc.group());
            // Keep a trailing separator when the user just typed "12." mid-entry.
            format!("{}{}{}{}", sign, grouped, loc.decimal(), frac)
        }
        None => format!("{}{}", sign, group_from_right(body, 3, loc.group())),
    }
}

/// Recompute the converted value and update the two display labels from the
/// current ConverterState. `top`/`bottom` are the two display labels.
fn converter_refresh(ui: &Ui, top: &gtk::Label, bottom: &gtk::Label) {
    let st = ui.converter.borrow();
    let units = st.category.units();
    let from = &units[st.from_idx.min(units.len() - 1)];
    let to = &units[st.to_idx.min(units.len() - 1)];
    let shown_input = if st.input.is_empty() { "0".to_string() } else { st.input.clone() };
    let localized_input = localize_decimal_string(&shown_input, ui.locale());
    top.set_text(&format!("{} {}", localized_input, from.symbol));
    let result = crate::convert::convert(st.category, from, to, st.value());
    if crate::convert::is_overflow(result) {
        bottom.add_css_class("calc-error");
        bottom.set_text("Overflow");
    } else {
        bottom.remove_css_class("calc-error");
        bottom.set_text(&format!(
            "{} {}",
            crate::engine::format::format_result_locale(result, ui.locale()),
            to.symbol
        ));
    }
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

/// Build the unit-converter page (reduced keypad, no OSK) and return its root
/// widget for insertion into the mode stack.
fn build_converter_page(ui: &Ui) -> gtk::Widget {
    let top_label = ui.conv_top_label.clone();
    let bottom_label = ui.conv_bottom_label.clone();

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
                display.clipboard().set_text(&crate::engine::format::format_result_locale(r, ui.locale()));
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
                display.clipboard().set_text(&crate::engine::format::format_result_locale(r, ui.locale()));
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

    converter_refresh(ui, &top_label, &bottom_label);
    clamp.upcast::<gtk::Widget>()
}

/// Group the digits of `s` into runs of `n` counted from the RIGHT, inserting
/// `sep` between runs. Any sign must be stripped by the caller. e.g.
/// `group_from_right("DEADBEEF", 4, ' ') == "DEAD BEEF"`.
fn group_from_right(s: &str, n: usize, sep: char) -> String {
    if s.is_empty() || n == 0 {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len + len / n);
    for (i, c) in chars.iter().enumerate() {
        // Insert a separator before a char whose distance from the right is a
        // positive multiple of n (i.e. at the start of a new left-side group).
        let from_right = len - i;
        if i != 0 && from_right % n == 0 {
            out.push(sep);
        }
        out.push(*c);
    }
    out
}

/// A programmer operator key: appends the `sym` token via press_op, re-renders.
fn prog_op_btn(ui: &Ui, label: &str, sym: &str, class: &str) -> gtk::Button {
    let sym = sym.to_string();
    let btn = gtk::Button::builder()
        .label(label)
        .css_classes(["calc-btn", "calc-btn-prog", class])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    btn.connect_clicked(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.prog.borrow_mut();
                st.press_op(&sym);
            }
            ui.render_prog();
        }
    ));
    btn
}

/// Assemble the financial-mode page: a calculator picker (ComboRow), a
/// dynamically-rebuilt group of per-field input rows, a result row, and a
/// reduced numeric keypad. Returns the page root widget.
#[allow(clippy::too_many_arguments)]
fn build_financial_page(
    ui: &Ui,
    calc_row: &adw::ComboRow,
    field_group: &adw::PreferencesGroup,
    result_row: &adw::ActionRow,
) -> gtk::Widget {
    // Seed the picker from persistence.
    let cur = settings::fin_calc();
    let cur_idx = crate::fin_state::FinCalc::all()
        .iter()
        .position(|c| *c == cur)
        .unwrap_or(0) as u32;
    calc_row.set_selected(cur_idx);

    calc_row.connect_selected_notify(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |row| {
            let all = crate::fin_state::FinCalc::all();
            let new_calc = all[(row.selected() as usize).min(all.len() - 1)];
            {
                let mut st = ui.fin.borrow_mut();
                st.select(new_calc);
            }
            settings::set_fin_calc(new_calc);
            ui.fin_rebuild_fields();
        }
    ));

    let calc_group = adw::PreferencesGroup::builder().build();
    calc_group.add(calc_row);

    let result_group = adw::PreferencesGroup::builder().build();
    result_group.add(result_row);

    // ── Reduced numeric keypad ───────────────────────────────────────────
    let pad = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(8)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(16)
        .build();

    // A keypad button whose action is a plain `fn` pointer mutating FinState.
    let fin_key = |label: &str, class: &str, mutate: fn(&mut crate::fin_state::FinState)| -> gtk::Button {
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
            #[upgrade_or_default]
            move |_| {
                {
                    let mut st = ui.fin.borrow_mut();
                    mutate(&mut st);
                }
                ui.render_fin();
            }
        ));
        btn
    };

    // A digit button (closure captures the char, so it's a separate builder).
    let digit = |d: char| -> gtk::Button {
        let btn = gtk::Button::builder()
            .label(d.to_string())
            .css_classes(["calc-btn", "calc-digit"])
            .hexpand(true)
            .vexpand(false)
            .can_focus(false)
            .build();
        btn.connect_clicked(clone!(
            #[weak]
            ui,
            #[upgrade_or_default]
            move |_| {
                {
                    let mut st = ui.fin.borrow_mut();
                    st.press_digit(d);
                }
                ui.render_fin();
            }
        ));
        btn
    };

    // Backspace (icon).
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
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.fin.borrow_mut();
                st.backspace();
            }
            ui.render_fin();
        }
    ));

    // Equals — compute is live; equals just refreshes.
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
        #[upgrade_or_default]
        move |_| {
            ui.render_fin();
        }
    ));

    // Row 0: 7 8 9 AC
    pad.attach(&digit('7'), 0, 0, 1, 1);
    pad.attach(&digit('8'), 1, 0, 1, 1);
    pad.attach(&digit('9'), 2, 0, 1, 1);
    pad.attach(&fin_key("AC", "calc-clear", |s| s.clear_all()), 3, 0, 1, 1);
    // Row 1: 4 5 6 ⌫
    pad.attach(&digit('4'), 0, 1, 1, 1);
    pad.attach(&digit('5'), 1, 1, 1, 1);
    pad.attach(&digit('6'), 2, 1, 1, 1);
    pad.attach(&back, 3, 1, 1, 1);
    // Row 2: 1 2 3 C
    pad.attach(&digit('1'), 0, 2, 1, 1);
    pad.attach(&digit('2'), 1, 2, 1, 1);
    pad.attach(&digit('3'), 2, 2, 1, 1);
    pad.attach(&fin_key("C", "calc-function", |s| s.clear_active()), 3, 2, 1, 1);
    // Row 3: 0 . ± =
    pad.attach(&digit('0'), 0, 3, 1, 1);
    pad.attach(&fin_key(".", "calc-digit", |s| s.press_dot()), 1, 3, 1, 1);
    pad.attach(&fin_key("\u{00B1}", "calc-function", |s| s.press_negate()), 2, 3, 1, 1);
    pad.attach(&equals, 3, 3, 1, 1);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(16)
        .build();
    content.append(&calc_group);
    content.append(field_group);
    content.append(&result_group);
    content.append(&pad);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();

    let clamp = adw::Clamp::builder()
        .maximum_size(480)
        .child(&scroller)
        .build();

    // Build the initial calc's field rows (also renders).
    ui.fin_rebuild_fields();
    clamp.upcast::<gtk::Widget>()
}

/// Assemble the programmer-mode page: a fixed-height base-display block (Hex/
/// Dec/Oct/Bin), a controls strip (bit-width segmented toggles + signed toggle),
/// the expression line, and a 5-column keypad. Returns the page root widget.
fn build_programmer_page(
    ui: &Ui,
    rows: Vec<gtk::Button>,
    hex_btns: Vec<gtk::Button>,
    digit_btns: Vec<gtk::Button>,
    width_btns: Vec<gtk::ToggleButton>,
    signed_btn: &gtk::ToggleButton,
    expr_label: &gtk::Label,
) -> gtk::Widget {
    // (A) Base display block — FIXED HEIGHT to keep the keypad pixel-stable.
    let base_block = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .height_request(150)
        .margin_start(20)
        .margin_end(20)
        .margin_top(8)
        .margin_bottom(4)
        .build();
    let base_specs = [Base::Hex, Base::Dec, Base::Oct, Base::Bin];
    for (row_btn, base) in rows.iter().zip(base_specs.iter()) {
        let base = *base;
        row_btn.connect_clicked(clone!(
            #[weak]
            ui,
            #[upgrade_or_default]
            move |_| {
                {
                    let mut st = ui.prog.borrow_mut();
                    st.set_base(base);
                }
                ui.prog_sync_digit_sensitivity();
                settings::set_prog_base(base);
                ui.render_prog();
            }
        ));
        base_block.append(row_btn);
    }

    // (B) Controls strip: width segmented control + signed toggle.
    let width_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    width_box.add_css_class("linked");
    // Group the toggles so exactly one is active at a time.
    let first = width_btns[0].clone();
    for btn in width_btns.iter().skip(1) {
        btn.set_group(Some(&first));
    }
    for (i, btn) in width_btns.iter().enumerate() {
        let w = [Width::W8, Width::W16, Width::W32, Width::W64][i];
        btn.connect_toggled(clone!(
            #[weak]
            ui,
            #[upgrade_or_default]
            move |b| {
                if !b.is_active() {
                    return; // ignore the deactivating half of the pair
                }
                let cur = { ui.prog.borrow().width() };
                if cur == w {
                    return; // guard against the render-driven set_active
                }
                {
                    let mut st = ui.prog.borrow_mut();
                    st.set_width(w);
                }
                settings::set_prog_width(w);
                ui.render_prog();
            }
        ));
        width_box.append(btn);
    }

    // Signed/unsigned toggle: active == signed.
    signed_btn.connect_toggled(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |b| {
            let want = b.is_active();
            let cur = { ui.prog.borrow().signed() };
            if cur == want {
                return;
            }
            {
                let mut st = ui.prog.borrow_mut();
                st.set_signed(want);
            }
            settings::set_prog_signed(want);
            ui.render_prog();
        }
    ));

    let controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    controls.append(&width_box);
    let spacer = gtk::Box::builder().hexpand(true).build();
    controls.append(&spacer);
    controls.append(signed_btn);

    // Expression / error line (small, dim, right-aligned).
    let expr_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(20)
        .margin_end(20)
        .build();
    expr_label.set_hexpand(true);
    expr_row.append(expr_label);

    // (C) Keypad — 5 columns.
    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(8)
        .row_homogeneous(true)
        .column_homogeneous(true)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(16)
        .build();

    // Hex digit keys A–F: wire press_digit and place them; store nothing new
    // (they already live in ui.prog_hex_btns for the sensitivity sync).
    let hex_chars = ['A', 'B', 'C', 'D', 'E', 'F'];
    for (btn, c) in hex_btns.iter().zip(hex_chars.iter()) {
        let c = *c;
        btn.connect_clicked(clone!(
            #[weak]
            ui,
            #[upgrade_or_default]
            move |_| {
                {
                    let mut st = ui.prog.borrow_mut();
                    st.press_digit(c);
                }
                ui.render_prog();
            }
        ));
    }

    // Wire the 0–9 digit keys (press_digit). Placed into the grid below.
    for (i, btn) in digit_btns.iter().enumerate() {
        let c = std::char::from_digit(i as u32, 10).unwrap();
        btn.connect_clicked(clone!(
            #[weak]
            ui,
            #[upgrade_or_default]
            move |_| {
                {
                    let mut st = ui.prog.borrow_mut();
                    st.press_digit(c);
                }
                ui.render_prog();
            }
        ));
    }

    // Backspace (icon) and AC (clear) and equals — built inline.
    let back = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .css_classes(["calc-btn", "calc-btn-prog", "calc-function"])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    back.connect_clicked(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.prog.borrow_mut();
                st.backspace();
            }
            ui.render_prog();
        }
    ));
    let ac = gtk::Button::builder()
        .label("AC")
        .css_classes(["calc-btn", "calc-btn-prog", "calc-clear"])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    ac.connect_clicked(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.prog.borrow_mut();
                st.clear();
            }
            ui.render_prog();
        }
    ));
    let equals = gtk::Button::builder()
        .label("=")
        .css_classes(["calc-btn", "calc-btn-prog", "calc-equals"])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    equals.connect_clicked(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.prog.borrow_mut();
                st.equals();
            }
            ui.render_prog();
        }
    ));

    // Grid layout (col, row); 5 columns wide.
    // Row 0: A  B  AND(&)  OR(|)   ⌫
    grid.attach(&hex_btns[0], 0, 0, 1, 1); // A
    grid.attach(&hex_btns[1], 1, 0, 1, 1); // B
    grid.attach(&prog_op_btn(ui, "AND", "&", "calc-function"), 2, 0, 1, 1);
    grid.attach(&prog_op_btn(ui, "OR", "|", "calc-function"), 3, 0, 1, 1);
    grid.attach(&back, 4, 0, 1, 1);
    // Row 1: C  D  XOR(^)  NOT(~)  AC
    grid.attach(&hex_btns[2], 0, 1, 1, 1); // C
    grid.attach(&hex_btns[3], 1, 1, 1, 1); // D
    grid.attach(&prog_op_btn(ui, "XOR", "^", "calc-function"), 2, 1, 1, 1);
    grid.attach(&prog_op_btn(ui, "NOT", "~", "calc-function"), 3, 1, 1, 1);
    grid.attach(&ac, 4, 1, 1, 1);
    // Row 2: E  F  <<  >>  ÷(/)
    grid.attach(&hex_btns[4], 0, 2, 1, 1); // E
    grid.attach(&hex_btns[5], 1, 2, 1, 1); // F
    grid.attach(&prog_op_btn(ui, "<<", "<<", "calc-function"), 2, 2, 1, 1);
    grid.attach(&prog_op_btn(ui, ">>", ">>", "calc-function"), 3, 2, 1, 1);
    grid.attach(&prog_op_btn(ui, "\u{00F7}", "/", "calc-operator"), 4, 2, 1, 1);
    // Row 3: 7 8 9 ( ×(*)
    grid.attach(&digit_btns[7], 0, 3, 1, 1);
    grid.attach(&digit_btns[8], 1, 3, 1, 1);
    grid.attach(&digit_btns[9], 2, 3, 1, 1);
    grid.attach(&prog_op_btn(ui, "(", "(", "calc-operator"), 3, 3, 1, 1);
    grid.attach(&prog_op_btn(ui, "\u{00D7}", "*", "calc-operator"), 4, 3, 1, 1);
    // Row 4: 4 5 6 ) −(-)
    grid.attach(&digit_btns[4], 0, 4, 1, 1);
    grid.attach(&digit_btns[5], 1, 4, 1, 1);
    grid.attach(&digit_btns[6], 2, 4, 1, 1);
    grid.attach(&prog_op_btn(ui, ")", ")", "calc-operator"), 3, 4, 1, 1);
    grid.attach(&prog_op_btn(ui, "\u{2212}", "-", "calc-operator"), 4, 4, 1, 1);
    // Row 5: 1 2 3 mod(%) +(+)
    grid.attach(&digit_btns[1], 0, 5, 1, 1);
    grid.attach(&digit_btns[2], 1, 5, 1, 1);
    grid.attach(&digit_btns[3], 2, 5, 1, 1);
    grid.attach(&prog_op_btn(ui, "mod", "%", "calc-operator"), 3, 5, 1, 1);
    grid.attach(&prog_op_btn(ui, "+", "+", "calc-operator"), 4, 5, 1, 1);
    // Row 6: 0 00 (blank) (blank) =
    grid.attach(&digit_btns[0], 0, 6, 1, 1);
    // "00": press '0' twice.
    let zz = gtk::Button::builder()
        .label("00")
        .css_classes(["calc-btn", "calc-btn-prog", "calc-digit"])
        .hexpand(true)
        .vexpand(false)
        .can_focus(false)
        .build();
    zz.connect_clicked(clone!(
        #[weak]
        ui,
        #[upgrade_or_default]
        move |_| {
            {
                let mut st = ui.prog.borrow_mut();
                st.press_digit('0');
                st.press_digit('0');
            }
            ui.render_prog();
        }
    ));
    grid.attach(&zz, 1, 6, 1, 1);
    grid.attach(&equals, 4, 6, 1, 1);

    // Assemble the page: base block (top), controls, expr, keypad (bottom).
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    page.append(&base_block);
    page.append(&controls);
    page.append(&expr_row);
    let keypad_wrap = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::End)
        .build();
    keypad_wrap.append(&grid);
    let keypad_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&keypad_wrap)
        .build();
    page.append(&keypad_scroller);

    // Initialize width + signed toggle active state from settings (guarded via
    // the toggled handlers' cur==want checks; render_prog also syncs later).
    let init_w = settings::prog_width();
    let init_idx = match init_w {
        Width::W8 => 0,
        Width::W16 => 1,
        Width::W32 => 2,
        Width::W64 => 3,
    };
    if let Some(btn) = width_btns.get(init_idx) {
        if !btn.is_active() {
            btn.set_active(true);
        }
    }
    let init_signed = settings::prog_signed();
    if signed_btn.is_active() != init_signed {
        signed_btn.set_active(init_signed);
    }

    let clamp = adw::Clamp::builder()
        .maximum_size(480)
        .child(&page)
        .build();
    clamp.upcast::<gtk::Widget>()
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

            let child = ui.content_stack.visible_child_name();
            match child.as_deref() {
                Some("programmer") => {
                    let mut handled = true;
                    {
                        let mut st = ui.prog.borrow_mut();
                        if let Some(ch) = keyval.to_unicode() {
                            let up = ch.to_ascii_uppercase();
                            match up {
                                '0'..='9' => st.press_digit(ch),
                                'A'..='F' => st.press_digit(up),
                                '&' => st.press_op("&"),
                                '|' => st.press_op("|"),
                                '^' => st.press_op("^"),
                                '~' => st.press_op("~"),
                                '+' => st.press_op("+"),
                                '-' => st.press_op("-"),
                                '*' => st.press_op("*"),
                                '/' => st.press_op("/"),
                                '%' => st.press_op("%"),
                                '(' => st.press_op("("),
                                ')' => st.press_op(")"),
                                '<' => st.press_op("<<"),
                                '>' => st.press_op(">>"),
                                '=' => {
                                    st.equals();
                                    drop(st);
                                    ui.render_prog();
                                    return glib::Propagation::Stop;
                                }
                                _ => handled = false,
                            }
                        } else {
                            handled = false;
                        }
                    }
                    if !handled {
                        match keyval {
                            gdk::Key::Return | gdk::Key::KP_Enter => {
                                {
                                    let mut st = ui.prog.borrow_mut();
                                    st.equals();
                                }
                                ui.render_prog();
                                return glib::Propagation::Stop;
                            }
                            gdk::Key::BackSpace => {
                                let mut st = ui.prog.borrow_mut();
                                st.backspace();
                            }
                            gdk::Key::Escape | gdk::Key::Delete => {
                                let mut st = ui.prog.borrow_mut();
                                st.clear();
                            }
                            _ => return glib::Propagation::Proceed,
                        }
                    }
                    ui.render_prog();
                    return glib::Propagation::Stop;
                }
                Some("converter") => {
                    if let Some(ch) = keyval.to_unicode() {
                        match ch {
                            '0'..='9' => {
                                {
                                    let mut st = ui.converter.borrow_mut();
                                    if st.input == "0" {
                                        st.input.clear();
                                    }
                                    st.input.push(ch);
                                }
                                converter_refresh(&ui, &ui.conv_top_label, &ui.conv_bottom_label);
                                return glib::Propagation::Stop;
                            }
                            '.' | ',' => {
                                {
                                    let mut st = ui.converter.borrow_mut();
                                    if st.input.is_empty() {
                                        st.input.push_str("0.");
                                    } else if !st.input.contains('.') {
                                        st.input.push('.');
                                    }
                                }
                                converter_refresh(&ui, &ui.conv_top_label, &ui.conv_bottom_label);
                                return glib::Propagation::Stop;
                            }
                            _ => {}
                        }
                    }
                    match keyval {
                        gdk::Key::BackSpace => {
                            {
                                ui.converter.borrow_mut().input.pop();
                            }
                            converter_refresh(&ui, &ui.conv_top_label, &ui.conv_bottom_label);
                            glib::Propagation::Stop
                        }
                        gdk::Key::Escape | gdk::Key::Delete => {
                            {
                                ui.converter.borrow_mut().input.clear();
                            }
                            converter_refresh(&ui, &ui.conv_top_label, &ui.conv_bottom_label);
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
                Some("financial") => {
                    if let Some(ch) = keyval.to_unicode() {
                        match ch {
                            '0'..='9' => {
                                { let mut st = ui.fin.borrow_mut(); st.press_digit(ch); }
                                ui.render_fin();
                                return glib::Propagation::Stop;
                            }
                            '.' | ',' => {
                                { let mut st = ui.fin.borrow_mut(); st.press_dot(); }
                                ui.render_fin();
                                return glib::Propagation::Stop;
                            }
                            '=' => {
                                ui.render_fin();
                                return glib::Propagation::Stop;
                            }
                            '-' => {
                                { let mut st = ui.fin.borrow_mut(); st.press_negate(); }
                                ui.render_fin();
                                return glib::Propagation::Stop;
                            }
                            _ => {}
                        }
                    }
                    match keyval {
                        gdk::Key::minus | gdk::Key::KP_Subtract => {
                            { let mut st = ui.fin.borrow_mut(); st.press_negate(); }
                            ui.render_fin();
                            glib::Propagation::Stop
                        }
                        gdk::Key::Return | gdk::Key::KP_Enter => {
                            ui.render_fin();
                            glib::Propagation::Stop
                        }
                        gdk::Key::BackSpace => {
                            { let mut st = ui.fin.borrow_mut(); st.backspace(); }
                            ui.render_fin();
                            glib::Propagation::Stop
                        }
                        gdk::Key::Escape | gdk::Key::Delete => {
                            { let mut st = ui.fin.borrow_mut(); st.clear_all(); }
                            ui.render_fin();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
                Some("calculator") => {
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
                _ => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(controller);
}
