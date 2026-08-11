//! Editing state machine for the calculator's *programmer mode*.
//!
//! [`ProgState`] sits directly on top of the pure [`crate::programmer`] engine
//! and turns keypresses into an editable expression. It is UI-agnostic: no GTK,
//! no signals, no formatting glue beyond what the engine already provides. All
//! numeric work is delegated to the engine via the `programmer::` path; this
//! module owns only the *editing* concerns (buffer, base, width, signedness,
//! and a latched error message).
//!
//! # The buffer model
//!
//! The core of the state is a single `String` buffer holding an in-progress
//! expression written in the currently active base. It is a flat token stream
//! of two kinds of characters:
//!
//! * **digits** of the active base — uppercase-normalized on entry, so a hex
//!   `a` is stored as `A`; and
//! * **operator / paren tokens** — exactly one of
//!   `& | ^ ~ << >> + - * / % ( )`.
//!
//! The buffer is **never grammar-checked on input**. `press_digit` only asks
//! the engine whether a character is a legal digit for the current base, and
//! `press_op` only checks that the symbol is one of the recognized operator
//! tokens. Anything else is silently ignored. Whether the resulting sequence
//! is a *valid expression* is decided solely by the engine when we evaluate it
//! (in [`ProgState::value`] and [`ProgState::equals`]). This keeps typing cheap
//! and lets partial expressions like `"5+"` exist transiently without error.
//!
//! # Live value evaluation
//!
//! [`ProgState::value`] live-evaluates the buffer through
//! [`crate::programmer::evaluate`] on every call. Two cases collapse to `None`
//! rather than surfacing an error:
//!
//! * an **empty** (or whitespace-only) buffer — there is simply no value yet;
//!   note the engine itself returns `Err(Syntax)` for an empty string, so we
//!   short-circuit before calling it; and
//! * a **parse error** — a half-typed expression such as `"5+"` is not a
//!   failure the user should see mid-type, so it is reported as "no value".
//!
//! Because of this, `value` is a `&self` getter and *never* sets `error`. The
//! only place an error is ever latched is [`ProgState::equals`].
//!
//! # Base switching collapses the live value
//!
//! Changing the base does not re-interpret the existing digits in place;
//! instead [`ProgState::set_base`] reads the current live value (under the old
//! base) and re-renders it as a digit string in the new base. So `"FF"` in
//! `Hex` becomes `"255"` after switching to `Dec`. If there is no live value
//! (empty or un-parseable buffer) the buffer is preserved as-is rather than
//! cleared. [`ProgState::set_width`] and [`ProgState::set_signed`] behave
//! similarly, but additionally re-mask the value to the new width / signedness.
//!
//! # Settings & error semantics
//!
//! * `error` is set **only** by [`ProgState::equals`], and only on an engine
//!   error; on success `equals` clears it.
//! * getters ([`value`](ProgState::value), [`display`](ProgState::display),
//!   [`expression`](ProgState::expression), [`error`](ProgState::error), and
//!   the `base` / `width` / `signed` accessors) never mutate `error`.
//! * every mutating `set_*`, `press_*`, `clear`, and `backspace` clears `error`.
//! * invalid input (an out-of-base digit, an unknown operator symbol) is
//!   silently ignored and does **not** set `error`.

use crate::programmer::{self, Base, Width};

/// The recognized operator / parenthesis tokens, in no particular order.
///
/// [`ProgState::press_op`] accepts a symbol only if it exactly matches one of
/// these; anything else is ignored. Kept as a single source of truth so the
/// press logic and this list cannot drift apart.
const OP_TOKENS: [&str; 13] = [
    "&", "|", "^", "~", "<<", ">>", "+", "-", "*", "/", "%", "(", ")",
];

/// Editable programmer-mode expression plus its display settings.
///
/// See the [module docs](self) for the buffer model and error semantics.
pub struct ProgState {
    /// The in-progress expression: active-base digits plus operator/paren
    /// tokens. Not grammar-validated on input.
    buffer: String,
    /// The base the buffer's digits are written in.
    base: Base,
    /// The fixed integer width used for evaluation and masking.
    width: Width,
    /// Whether values are interpreted / displayed as signed.
    signed: bool,
    /// The last error from [`ProgState::equals`], if any. Only `equals` sets
    /// this; every other mutator clears it.
    error: Option<String>,
}

impl ProgState {
    /// Creates a fresh state with an empty buffer, the given display settings,
    /// and no error.
    pub fn new(base: Base, width: Width, signed: bool) -> Self {
        Self {
            buffer: String::new(),
            base,
            width,
            signed,
            error: None,
        }
    }

    /// Appends a digit character, but only if it is valid for the active base.
    ///
    /// Validity is checked against the *original* `c` (the engine's
    /// `is_valid_digit` is case-insensitive for hex). On a valid digit the
    /// error is cleared and the character is stored uppercase-normalized. An
    /// invalid digit is silently ignored and does not set an error.
    pub fn press_digit(&mut self, c: char) {
        if self.base.is_valid_digit(c) {
            self.error = None;
            self.buffer.push(c.to_ascii_uppercase());
        }
    }

    /// Appends an operator / parenthesis token if `sym` is a recognized token.
    ///
    /// Accepts exactly `& | ^ ~ << >> + - * / % ( )`; unknown symbols are
    /// ignored silently. Does not validate grammar — the literal token string
    /// is appended as-is. Clears the error on a recognized token.
    pub fn press_op(&mut self, sym: &str) {
        if OP_TOKENS.contains(&sym) {
            self.error = None;
            self.buffer.push_str(sym);
        }
    }

    /// All-clear: empties the buffer and clears any error.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.error = None;
    }

    /// Removes the last token from the buffer.
    ///
    /// A trailing two-character shift token (`<<` or `>>`) is removed whole;
    /// otherwise a single character is popped. Does nothing (no panic) when the
    /// buffer is already empty. Clears the error.
    pub fn backspace(&mut self) {
        if !self.buffer.is_empty() {
            if self.buffer.ends_with("<<") || self.buffer.ends_with(">>") {
                self.buffer.truncate(self.buffer.len() - 2);
            } else {
                self.buffer.pop();
            }
        }
        self.error = None;
    }

    /// Evaluates the buffer and collapses it to the formatted result.
    ///
    /// An empty (or whitespace-only) buffer is a no-op and does not set an
    /// error. On a successful evaluation the buffer is replaced with the
    /// formatted value and the error is cleared. On an engine error the buffer
    /// is left untouched and the error message is latched.
    pub fn equals(&mut self) {
        if self.buffer.trim().is_empty() {
            return;
        }
        match programmer::evaluate(&self.buffer, self.base, self.width, self.signed) {
            Ok(v) => {
                self.buffer = programmer::format(v, self.base, self.width, self.signed);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    /// Switches the active base. A complete live value is collapsed into the new
    /// base's digit string. A partial (un-parseable) buffer is **preserved**
    /// only when every one of its digit characters is valid in the destination
    /// base, so an in-progress expression survives the switch (its digits are
    /// reinterpreted going forward); otherwise it would leave digits the new
    /// base can't lex, so it is **cleared** instead.
    ///
    /// No-op if the base is unchanged. The live value is read *before* the base
    /// changes. Clears the error.
    pub fn set_base(&mut self, base: Base) {
        if base == self.base {
            return;
        }
        if let Some(v) = self.value() {
            // A complete value collapses to its rendering in the new base.
            self.buffer = programmer::format(v, base, self.width, self.signed);
        } else if !self
            .buffer
            .chars()
            .filter(|c| c.is_alphanumeric())
            .all(|c| base.is_valid_digit(c))
        {
            // A partial buffer carrying digits invalid in the new base would be
            // unlexable, so drop it. Operator chars are not digits and ignored.
            self.buffer.clear();
        }
        self.base = base;
        self.error = None;
    }

    /// Changes the fixed width, re-masking the current live value to it.
    ///
    /// The live value is read under the *old* width, then the width changes; if
    /// a value was present it is masked to the new width and re-rendered in the
    /// active base. Clears the error.
    pub fn set_width(&mut self, width: Width) {
        let v = self.value();
        self.width = width;
        if let Some(v) = v {
            let m = programmer::masked(v, width, self.signed);
            self.buffer = programmer::format(m, self.base, width, self.signed);
        }
        self.error = None;
    }

    /// Toggles signed interpretation, re-masking the current live value.
    ///
    /// The live value is read under the old signedness, then signedness
    /// changes; a present value is re-masked (sign-extending as needed) and
    /// re-rendered. Clears the error.
    pub fn set_signed(&mut self, signed: bool) {
        let v = self.value();
        self.signed = signed;
        if let Some(v) = v {
            let m = programmer::masked(v, self.width, signed);
            self.buffer = programmer::format(m, self.base, self.width, signed);
        }
        self.error = None;
    }

    /// Returns the active base.
    pub fn base(&self) -> Base {
        self.base
    }

    /// Returns the active width.
    pub fn width(&self) -> Width {
        self.width
    }

    /// Returns whether values are interpreted as signed.
    pub fn signed(&self) -> bool {
        self.signed
    }

    /// Live-evaluates the buffer, returning `None` for an empty buffer or a
    /// parse error.
    ///
    /// This is a pure getter: it never sets `error`, so partial expressions
    /// typed mid-stream stay silent.
    pub fn value(&self) -> Option<i128> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        programmer::evaluate(&self.buffer, self.base, self.width, self.signed).ok()
    }

    /// Live-evaluates the buffer to surface a *genuine arithmetic* error while
    /// typing, without flagging partial (mid-type) syntax.
    ///
    /// An empty (or whitespace-only) buffer, or a `Syntax` error (a half-typed
    /// expression such as `"5+"`), stays silent and returns `None`. A complete
    /// expression that hits `DivideByZero` or `Overflow` returns `Some(msg)` so
    /// the UI can surface it before `=` is pressed. Never sets `error`.
    pub fn error_preview(&self) -> Option<String> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        match programmer::evaluate(&self.buffer, self.base, self.width, self.signed) {
            Ok(_) => None,
            Err(programmer::ProgError::Syntax) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    /// Renders the current live value in `base` for display.
    ///
    /// When there is no live value, `0` is rendered instead, so a display panel
    /// always shows a well-formed number.
    pub fn display(&self, base: Base) -> String {
        match self.value() {
            Some(v) => programmer::format(v, base, self.width, self.signed),
            None => programmer::format(0, base, self.width, self.signed),
        }
    }

    /// Returns a copy of the raw expression buffer.
    pub fn expression(&self) -> String {
        self.buffer.clone()
    }

    /// Returns the latched error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programmer::{Base, Width};

    #[test]
    fn hex_ff_value_and_displays() {
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        s.press_digit('F');
        s.press_digit('F');
        assert_eq!(s.value(), Some(255));
        assert_eq!(s.display(Base::Dec), "255");
        assert_eq!(s.display(Base::Bin), "11111111");
        assert_eq!(s.display(Base::Hex), "FF");
    }

    #[test]
    fn bin_1010_value() {
        let mut s = ProgState::new(Base::Bin, Width::W8, false);
        for c in "1010".chars() {
            s.press_digit(c);
        }
        assert_eq!(s.value(), Some(10));
        assert_eq!(s.display(Base::Dec), "10");
    }

    #[test]
    fn base_switch_hex_to_dec_collapses() {
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        for c in "FF".chars() {
            s.press_digit(c);
        }
        s.set_base(Base::Dec);
        assert_eq!(s.expression(), "255");
        assert_eq!(s.value(), Some(255));
    }

    #[test]
    fn width_change_remasks() {
        let mut s = ProgState::new(Base::Hex, Width::W16, false);
        for c in "1FF".chars() {
            s.press_digit(c);
        }
        assert_eq!(s.value(), Some(511));
        s.set_width(Width::W8);
        assert_eq!(s.value(), Some(255));
        assert_eq!(s.expression(), "FF");
    }

    #[test]
    fn bitwise_ops_via_press() {
        // AND
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        for c in "F0".chars() {
            s.press_digit(c);
        }
        s.press_op("&");
        for c in "0F".chars() {
            s.press_digit(c);
        }
        assert_eq!(s.value(), Some(0));

        // OR
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        for c in "F0".chars() {
            s.press_digit(c);
        }
        s.press_op("|");
        for c in "0F".chars() {
            s.press_digit(c);
        }
        assert_eq!(s.value(), Some(255));

        // XOR
        let mut s = ProgState::new(Base::Bin, Width::W8, false);
        for c in "1010".chars() {
            s.press_digit(c);
        }
        s.press_op("^");
        for c in "0110".chars() {
            s.press_digit(c);
        }
        assert_eq!(s.value(), Some(12));

        // shift
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.press_op("<<");
        s.press_digit('2');
        assert_eq!(s.value(), Some(20));
    }

    #[test]
    fn signed_negative_one() {
        let mut s = ProgState::new(Base::Dec, Width::W8, true);
        s.press_op("-");
        s.press_digit('1');
        assert_eq!(s.value(), Some(-1));
        assert_eq!(s.display(Base::Dec), "-1");
        assert_eq!(s.display(Base::Hex), "FF");
    }

    #[test]
    fn divide_by_zero_sets_error_keeps_buffer() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.press_op("/");
        s.press_digit('0');
        s.equals();
        assert!(s.error().is_some());
        assert!(s.expression().contains("5") && s.expression().contains("/") && s.expression().contains("0"));
    }

    #[test]
    fn invalid_digit_rejected() {
        // Oct: 9 and 8 are out of range.
        let mut s = ProgState::new(Base::Oct, Width::W8, false);
        s.press_digit('9');
        s.press_digit('8');
        assert!(s.expression().is_empty());

        // Bin: 2 is out of range.
        let mut s = ProgState::new(Base::Bin, Width::W8, false);
        s.press_digit('2');
        assert!(s.expression().is_empty());

        // Dec: F is out of range.
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('F');
        assert!(s.expression().is_empty());

        // Hex: F is accepted.
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        s.press_digit('F');
        assert_eq!(s.expression(), "F");
    }

    #[test]
    fn backspace_removes_shift_token() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('1');
        s.press_op("<<");
        s.backspace();
        assert_eq!(s.expression(), "1");
    }

    #[test]
    fn clear_resets() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.clear();
        assert!(s.expression().is_empty() && s.error().is_none());
    }

    #[test]
    fn equals_collapses() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('2');
        s.press_op("+");
        s.press_digit('3');
        s.equals();
        assert_eq!(s.expression(), "5");
        assert_eq!(s.value(), Some(5));
    }

    #[test]
    fn empty_buffer_semantics() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        assert_eq!(s.value(), None);
        assert_eq!(s.display(Base::Dec), "0");
        s.equals();
        assert!(s.error().is_none() && s.expression().is_empty());
    }

    #[test]
    fn hex_letter_case_normalized() {
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        s.press_digit('a');
        assert_eq!(s.expression(), "A");
    }

    #[test]
    fn divide_by_zero_live_surfaces_error() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.press_op("/");
        s.press_digit('0');
        // Complete arithmetic error: surfaced live, before pressing =.
        assert_eq!(s.error_preview(), Some("Can't divide by 0".to_string()));
        // A partial expression stays silent.
        let mut s2 = ProgState::new(Base::Dec, Width::W8, false);
        s2.press_digit('5');
        s2.press_op("+");
        assert_eq!(s2.error_preview(), None);
    }

    #[test]
    fn partial_expr_live_is_silent() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.press_op("+");
        assert_eq!(s.error_preview(), None);
    }

    #[test]
    fn set_base_preserves_partial_buffer() {
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('5');
        s.press_op("+");
        assert_eq!(s.value(), None);
        s.set_base(Base::Hex);
        assert_eq!(s.expression(), "5+");
        assert_eq!(s.base(), Base::Hex);
    }

    #[test]
    fn set_base_preserves_partial_when_digits_valid() {
        // `5+` in Hex -> Dec: `5` is a valid Dec digit, so the partial buffer
        // survives the switch unchanged.
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        s.press_digit('5');
        s.press_op("+");
        s.set_base(Base::Dec);
        assert_eq!(s.expression(), "5+");
        assert_eq!(s.base(), Base::Dec);
    }

    #[test]
    fn set_base_clears_partial_with_invalid_digit() {
        // `A+` in Hex -> Dec: `A` is not a Dec digit and the buffer is not a
        // complete value, so it is cleared rather than left unlexable.
        let mut s = ProgState::new(Base::Hex, Width::W8, false);
        s.press_digit('A');
        s.press_op("+");
        s.set_base(Base::Dec);
        assert_eq!(s.expression(), "");
        assert_eq!(s.base(), Base::Dec);
        assert_eq!(s.value(), None);
    }

    #[test]
    fn set_base_clears_dec_partial_invalid_in_bin() {
        // `9+` in Dec -> Bin: `9` is not a Bin digit and the buffer is partial,
        // so it is cleared.
        let mut s = ProgState::new(Base::Dec, Width::W8, false);
        s.press_digit('9');
        s.press_op("+");
        s.set_base(Base::Bin);
        assert_eq!(s.expression(), "");
    }
}

