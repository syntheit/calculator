//! Editing state for the calculator's *financial mode*.
//!
//! [`FinState`] sits directly on top of the pure [`crate::financial`] engine and
//! turns keypresses into a small set of editable numeric fields. It is
//! UI-agnostic: no GTK, no signals, no formatting glue. Each financial
//! calculation ([`FinCalc`]) declares its own ordered list of input
//! [`FinField`]s; the state owns one raw decimal string per field plus the index
//! of the currently active field, and exposes a `press_*` keypad API that mirrors
//! the sibling programmer-mode state.
//!
//! # The field model
//!
//! The active calculation fixes how many fields exist and what each means. The
//! state stores their values as raw strings (e.g. `"1000"`, `"-4.5"`, `""`) so a
//! half-typed entry like `"-"` or `"0."` can exist transiently without error.
//! Nothing is grammar-checked on input; parsing to a number happens lazily in
//! [`FinState::parse_field`], which treats every empty or partial string as
//! `0.0`.
//!
//! # The percent convention
//!
//! Rate fields are entered *as a percent*: the user types `5` to mean `5%`.
//! [`FinState::compute`] divides those rate fields by `100.0` before handing them
//! to the engine, which works in plain decimal rates. Non-rate fields (amounts,
//! counts, periods) are passed through unchanged.

use crate::financial::FinError;

/// Describes one input row of a financial calculator: the internal key, the
/// human label shown at the row's start, and a short dimmed unit suffix.
#[derive(Clone, Copy, Debug)]
pub struct FinField {
    /// Stable internal identifier for the field (not shown in the UI; kept so
    /// field sets are self-describing and future logic can key off it).
    #[allow(dead_code)]
    pub key: &'static str,
    pub label: &'static str,
    pub suffix: &'static str,
}

/// The input fields for [`FinCalc::Compound`].
const COMPOUND_FIELDS: [FinField; 3] = [
    FinField { key: "pv", label: "Present value", suffix: "$" },
    FinField { key: "rate", label: "Rate / period", suffix: "%" },
    FinField { key: "n", label: "Periods", suffix: "" },
];

/// The input fields for [`FinCalc::Loan`].
const LOAN_FIELDS: [FinField; 3] = [
    FinField { key: "principal", label: "Loan amount", suffix: "$" },
    FinField { key: "rate", label: "Rate / period", suffix: "%" },
    FinField { key: "n", label: "Periods", suffix: "" },
];

/// The input fields for [`FinCalc::Simple`].
const SIMPLE_FIELDS: [FinField; 3] = [
    FinField { key: "principal", label: "Principal", suffix: "$" },
    FinField { key: "rate", label: "Rate / period", suffix: "%" },
    FinField { key: "t", label: "Periods", suffix: "" },
];

/// The input fields for [`FinCalc::Margin`].
const MARGIN_FIELDS: [FinField; 2] = [
    FinField { key: "cost", label: "Cost", suffix: "$" },
    FinField { key: "margin", label: "Margin", suffix: "%" },
];

/// The input fields for [`FinCalc::Depreciation`].
const DEPRECIATION_FIELDS: [FinField; 4] = [
    FinField { key: "cost", label: "Cost", suffix: "$" },
    FinField { key: "salvage", label: "Salvage value", suffix: "$" },
    FinField { key: "life", label: "Useful life", suffix: "periods" },
    FinField { key: "period", label: "Period", suffix: "" },
];

/// The set of financial calculations the mode offers.
///
/// Each variant fixes a title, a stable key, a result label, and an ordered set
/// of input [`FinField`]s consumed by [`FinState::compute`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinCalc {
    /// Compound future value.
    Compound,
    /// Amortizing loan payment per period.
    Loan,
    /// Simple interest earned.
    Simple,
    /// Sale price from a gross margin.
    Margin,
    /// Sum-of-years'-digits depreciation for a period.
    Depreciation,
}

impl FinCalc {
    /// Every calculation, in display order.
    pub fn all() -> [FinCalc; 5] {
        [
            FinCalc::Compound,
            FinCalc::Loan,
            FinCalc::Simple,
            FinCalc::Margin,
            FinCalc::Depreciation,
        ]
    }

    /// The human title shown for this calculation.
    pub fn title(self) -> &'static str {
        match self {
            FinCalc::Compound => "Compound Interest",
            FinCalc::Loan => "Loan Payment",
            FinCalc::Simple => "Simple Interest",
            FinCalc::Margin => "Gross Margin",
            FinCalc::Depreciation => "Depreciation",
        }
    }

    /// The stable key used for persistence and [`FinCalc::from_key`].
    pub fn key(self) -> &'static str {
        match self {
            FinCalc::Compound => "compound",
            FinCalc::Loan => "loan",
            FinCalc::Simple => "simple",
            FinCalc::Margin => "margin",
            FinCalc::Depreciation => "depreciation",
        }
    }

    /// Parses a key back into a calculation, defaulting to [`FinCalc::Compound`]
    /// on an unknown string.
    pub fn from_key(s: &str) -> FinCalc {
        match s {
            "compound" => FinCalc::Compound,
            "loan" => FinCalc::Loan,
            "simple" => FinCalc::Simple,
            "margin" => FinCalc::Margin,
            "depreciation" => FinCalc::Depreciation,
            _ => FinCalc::Compound,
        }
    }

    /// The label shown next to this calculation's computed result.
    pub fn result_label(self) -> &'static str {
        match self {
            FinCalc::Compound => "Future value",
            FinCalc::Loan => "Payment / period",
            FinCalc::Simple => "Interest",
            FinCalc::Margin => "Sale price",
            FinCalc::Depreciation => "Depreciation (SYD)",
        }
    }

    /// This calculation's ordered input fields.
    pub fn fields(self) -> &'static [FinField] {
        match self {
            FinCalc::Compound => &COMPOUND_FIELDS,
            FinCalc::Loan => &LOAN_FIELDS,
            FinCalc::Simple => &SIMPLE_FIELDS,
            FinCalc::Margin => &MARGIN_FIELDS,
            FinCalc::Depreciation => &DEPRECIATION_FIELDS,
        }
    }
}

/// Editable financial-mode inputs plus the active-field cursor.
///
/// See the [module docs](self) for the field model and the percent convention.
pub struct FinState {
    /// The active calculation, which fixes the field set.
    selected: FinCalc,
    /// One raw decimal string per field of `selected`; `""` parses to `0`.
    values: Vec<String>,
    /// Index of the currently active field within `values`.
    active: usize,
}

impl FinState {
    /// Creates a fresh state for `selected` with every field empty and the
    /// first field active.
    pub fn new(selected: FinCalc) -> Self {
        Self {
            values: vec![String::new(); selected.fields().len()],
            selected,
            active: 0,
        }
    }

    /// Switches to `calc`, resetting all fields to empty and the cursor to the
    /// first field.
    pub fn select(&mut self, calc: FinCalc) {
        self.selected = calc;
        self.values = vec![String::new(); calc.fields().len()];
        self.active = 0;
    }

    /// Moves the active-field cursor, clamped to the last valid index.
    pub fn set_active(&mut self, idx: usize) {
        self.active = idx.min(self.values.len().saturating_sub(1));
    }

    /// Returns the active-field index.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Returns the active calculation.
    pub fn selected(&self) -> FinCalc {
        self.selected
    }

    /// Returns the raw string for field `idx`, or `""` if out of range.
    pub fn field_value(&self, idx: usize) -> &str {
        self.values.get(idx).map(|s| s.as_str()).unwrap_or("")
    }

    /// Appends a digit to the active field.
    ///
    /// Non-digit characters are ignored. A lone leading `0` is replaced by the
    /// new digit so `0` then `5` yields `5`, not `05`.
    pub fn press_digit(&mut self, c: char) {
        if !c.is_ascii_digit() {
            return;
        }
        if let Some(v) = self.values.get_mut(self.active) {
            if v == "0" {
                v.clear();
            }
            v.push(c);
        }
    }

    /// Appends a decimal point to the active field.
    ///
    /// An empty field becomes `"0."`; an existing point blocks a second one.
    pub fn press_dot(&mut self) {
        if let Some(v) = self.values.get_mut(self.active) {
            if v.is_empty() {
                v.push_str("0.");
            } else if !v.contains('.') {
                v.push('.');
            }
        }
    }

    /// Toggles the sign of the active field.
    ///
    /// Removes a leading `-` if present, otherwise inserts one at the front
    /// (even for an empty field).
    pub fn press_negate(&mut self) {
        if let Some(v) = self.values.get_mut(self.active) {
            if v.starts_with('-') {
                v.remove(0);
            } else {
                v.insert(0, '-');
            }
        }
    }

    /// Removes the last character of the active field (no-op if empty).
    pub fn backspace(&mut self) {
        if let Some(v) = self.values.get_mut(self.active) {
            v.pop();
        }
    }

    /// Clears the active field only.
    pub fn clear_active(&mut self) {
        if let Some(v) = self.values.get_mut(self.active) {
            v.clear();
        }
    }

    /// Clears every field and returns the cursor to the first field.
    pub fn clear_all(&mut self) {
        for v in &mut self.values {
            v.clear();
        }
        self.active = 0;
    }

    /// Parses field `idx` to `f64`, mapping empty or partial strings (`""`,
    /// `"-"`, `"."`, `"-."`) to `0.0`.
    pub fn parse_field(&self, idx: usize) -> f64 {
        self.field_value(idx).parse::<f64>().unwrap_or(0.0)
    }

    /// Computes the active calculation from the current fields.
    ///
    /// Rate fields are entered as a percent (the user types `5` for `5%`), so
    /// they are divided by `100.0` before being passed to the engine. Empty or
    /// partial fields contribute `0.0` via [`FinState::parse_field`].
    pub fn compute(&self) -> Result<f64, FinError> {
        match self.selected {
            FinCalc::Compound => {
                let pv = self.parse_field(0);
                let rate = self.parse_field(1) / 100.0;
                let n = self.parse_field(2);
                crate::financial::compound_fv(pv, rate, n)
            }
            FinCalc::Loan => {
                let principal = self.parse_field(0);
                let rate = self.parse_field(1) / 100.0;
                let n = self.parse_field(2);
                crate::financial::loan_payment(principal, rate, n)
            }
            FinCalc::Simple => {
                let principal = self.parse_field(0);
                let rate = self.parse_field(1) / 100.0;
                let t = self.parse_field(2);
                crate::financial::simple_interest(principal, rate, t)
            }
            FinCalc::Margin => {
                let cost = self.parse_field(0);
                let margin = self.parse_field(1) / 100.0;
                crate::financial::gross_margin_price(cost, margin)
            }
            FinCalc::Depreciation => {
                let cost = self.parse_field(0);
                let salvage = self.parse_field(1);
                let life = self.parse_field(2);
                let period = self.parse_field(3);
                crate::financial::depreciation_syd(cost, salvage, life, period)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// Types `text` into field `idx`, routing `.` and `-` through the dedicated
    /// keypad methods.
    fn enter(s: &mut FinState, idx: usize, text: &str) {
        s.set_active(idx);
        for c in text.chars() {
            if c == '.' {
                s.press_dot();
            } else if c == '-' {
                s.press_negate();
            } else {
                s.press_digit(c);
            }
        }
    }

    #[test]
    fn select_resets() {
        let mut s = FinState::new(FinCalc::Compound);
        s.press_digit('1');
        s.press_digit('2');
        s.select(FinCalc::Loan);
        assert_eq!(s.selected(), FinCalc::Loan);
        for i in 0..FinCalc::Loan.fields().len() {
            assert_eq!(s.field_value(i), "");
        }
        assert_eq!(s.active(), 0);
    }

    #[test]
    fn press_digit_uses_active() {
        let mut s = FinState::new(FinCalc::Compound);
        s.set_active(1);
        s.press_digit('5');
        s.press_digit('0');
        assert_eq!(s.field_value(1), "50");
        assert_eq!(s.field_value(0), "");
    }

    #[test]
    fn press_digit_replaces_leading_zero() {
        let mut s = FinState::new(FinCalc::Compound);
        s.press_digit('0');
        s.press_digit('5');
        assert_eq!(s.field_value(0), "5");
    }

    #[test]
    fn press_dot_and_negate() {
        let mut s = FinState::new(FinCalc::Compound);
        s.set_active(0);
        s.press_dot();
        assert_eq!(s.field_value(0), "0.");

        let mut s = FinState::new(FinCalc::Compound);
        s.set_active(0);
        s.press_digit('4');
        s.press_negate();
        assert_eq!(s.field_value(0), "-4");
        s.press_negate();
        assert_eq!(s.field_value(0), "4");
    }

    #[test]
    fn compound_compute() {
        let mut s = FinState::new(FinCalc::Compound);
        enter(&mut s, 0, "1000");
        enter(&mut s, 1, "5");
        enter(&mut s, 2, "10");
        let got = s.compute().unwrap();
        assert!(close(got, 1628.894627, 1e-3), "got {got}");
    }

    #[test]
    fn loan_compute() {
        let mut s = FinState::new(FinCalc::Loan);
        enter(&mut s, 0, "10000");
        enter(&mut s, 1, "0.416666667");
        enter(&mut s, 2, "360");
        let got = s.compute().unwrap();
        assert!(close(got, 53.6822, 0.02), "got {got}");
    }

    #[test]
    fn margin_compute() {
        let mut s = FinState::new(FinCalc::Margin);
        enter(&mut s, 0, "70");
        enter(&mut s, 1, "30");
        let got = s.compute().unwrap();
        assert!(close(got, 100.0, 1e-6), "got {got}");
    }

    #[test]
    fn depreciation_compute() {
        let mut s = FinState::new(FinCalc::Depreciation);
        enter(&mut s, 0, "1000");
        enter(&mut s, 1, "100");
        enter(&mut s, 2, "5");
        enter(&mut s, 3, "1");
        let got = s.compute().unwrap();
        assert!(close(got, 300.0, 1e-6), "got {got}");
    }

    #[test]
    fn clear_all_resets() {
        let mut s = FinState::new(FinCalc::Compound);
        enter(&mut s, 0, "123");
        enter(&mut s, 1, "45");
        enter(&mut s, 2, "6");
        s.clear_all();
        for i in 0..FinCalc::Compound.fields().len() {
            assert_eq!(s.field_value(i), "");
        }
        assert_eq!(s.active(), 0);
    }
}
