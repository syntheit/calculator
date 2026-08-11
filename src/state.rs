//! The calculator state machine — the API the GTK UI binds its buttons to.
//!
//! This module is deliberately UI-free. It owns the *canonical* expression the
//! [`crate::engine`] evaluates, bridges it to pretty display glyphs, and exposes
//! a small, documented surface that a separate UI agent builds against.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! PUBLIC SURFACE (stable — the UI agent codes to exactly this)
//! ─────────────────────────────────────────────────────────────────────────
//!
//! Types:
//! * `enum Op { Add, Sub, Mul, Div }`
//! * `enum Func { Sin, Cos, Tan, Ln, Log }`
//! * `enum CalcState { Input, Result, Error }`
//! * `struct Calculator`
//! * re-exports: `AngleUnit` (from engine), `HistoryEntry` (from history)
//!
//! Construction:
//! * `Calculator::new(angle: AngleUnit) -> Self`
//!
//! Input (build the expression):
//! * `press_digit(&mut self, c: char)`     — a digit `'0'..='9'`
//! * `press_dot(&mut self)`                — decimal point
//! * `press_op(&mut self, op: Op)`         — + − × ÷
//! * `press_power(&mut self)`              — `^`
//! * `press_paren(&mut self)`              — SMART `(` / `)`
//! * `press_percent(&mut self)`            — `%`
//! * `press_factorial(&mut self)`          — `!`
//! * `press_sqrt(&mut self)`               — `√` (or `x²` when `inv` on)
//! * `press_pi(&mut self)` / `press_e(&mut self)`
//! * `press_func(&mut self, f: Func)`      — `sin(` … (inverse form when `inv`)
//!
//! Control:
//! * `clear(&mut self)`                    — AC, wipe to empty Input
//! * `backspace(&mut self)`                — delete last TOKEN (smart)
//! * `equals(&mut self) -> Option<HistoryEntry>`
//!
//! Toggles:
//! * `toggle_inv(&mut self)` / `inv(&self) -> bool`
//! * `set_angle(&mut self, a: AngleUnit)` / `angle(&self) -> AngleUnit`
//!
//! Memory:
//! * `memory_store(&mut self)` / `memory_recall(&mut self)`
//! * `memory_add(&mut self)` / `memory_sub(&mut self)`
//! * `memory_clear(&mut self)` / `has_memory(&self) -> bool`
//!
//! Readout (for rendering):
//! * `display_expression(&self) -> String`   — pretty (× ÷ − √ π e …)
//! * `live_result(&self) -> Option<String>`  — instant preview, never an error
//! * `state(&self) -> CalcState`
//! * `error_message(&self) -> Option<String>`
//! * `current_value(&self) -> Option<f64>`   — numeric value for copy/memory
//!
//! ─────────────────────────────────────────────────────────────────────────

use crate::engine::{self, AngleUnit};
use crate::history::HistoryEntry;

/// A binary operator the UI can press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    /// The canonical (ASCII) glyph stored in the buffer.
    fn canonical(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
        }
    }
}

/// A named function button. The inverse (second-function) forms are produced by
/// [`Calculator::press_func`] when [`Calculator::inv`] is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Sin,
    Cos,
    Tan,
    Ln,
    Log,
    Sinh,
    Cosh,
    Tanh,
}

/// The three top-level modes of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcState {
    /// The user is building an expression.
    Input,
    /// `=` produced a value; the display shows it and it can seed the next expr.
    Result,
    /// `=` produced an error; [`Calculator::error_message`] is set.
    Error,
}

/// A single canonical token as-typed. Grouping tokens this way (rather than a
/// flat `String`) makes token-granular backspace and pretty display trivial.
#[derive(Clone, Debug, PartialEq)]
enum Chunk {
    /// A numeric literal being accumulated (`"3"`, `"3.1"`, `"3."`).
    Number(String),
    /// A single-glyph operator/paren/marker: one of `+ - * / ^ ( ) √ % ! π e`.
    Sym(&'static str),
    /// A function-plus-open-paren, e.g. `"sin("`, `"asin("`. The `10^` inverse
    /// of `log` is stored as two chunks (`Number`-less): a `Sym("10")`-like…
    /// actually as a dedicated variant so display and backspace treat it as one.
    Func(&'static str),
}

/// The calculator state machine.
pub struct Calculator {
    /// The canonical expression, one [`Chunk`] per logical token.
    buf: Vec<Chunk>,
    /// Trig angle unit.
    angle: AngleUnit,
    /// Scientific "inverse" (2nd-function) toggle.
    inv: bool,
    /// The memory register (`None` when empty).
    memory: Option<f64>,
    /// The last committed `=` result (numeric + formatted), if any.
    last_result: Option<(f64, String)>,
    /// The last committed expression display, kept for the Result readout.
    last_expr_display: Option<String>,
    /// Current display mode.
    state: CalcState,
    /// The message set when [`state`] is [`CalcState::Error`].
    error: Option<String>,
}

impl Calculator {
    // ---- construction ------------------------------------------------------

    /// Create an empty calculator in [`CalcState::Input`].
    pub fn new(angle: AngleUnit) -> Self {
        Self {
            buf: Vec::new(),
            angle,
            inv: false,
            memory: None,
            last_result: None,
            last_expr_display: None,
            state: CalcState::Input,
            error: None,
        }
    }

    // ---- small helpers -----------------------------------------------------

    /// True if the buffer's last chunk ends a *value* (a number, `)`, constant,
    /// `%` or `!`) — i.e. a following operator is legal and a following value
    /// implies multiplication.
    fn last_ends_value(&self) -> bool {
        match self.buf.last() {
            Some(Chunk::Number(_)) => true,
            Some(Chunk::Sym(s)) => matches!(*s, ")" | "π" | "e" | "%" | "!"),
            _ => false,
        }
    }

    /// True if the buffer's last chunk is a binary operator (`+ - * / ^`).
    fn last_is_binary_op(&self) -> bool {
        matches!(self.buf.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^"))
    }

    /// Reset to a fresh empty Input state, clearing any result/error flags.
    fn reset_input(&mut self) {
        self.buf.clear();
        self.state = CalcState::Input;
        self.error = None;
    }

    /// If we are in a Result/Error state, decide how a new keypress starts.
    ///
    /// * A value-opener (digit, dot, const, func, `√`, `(`) after a Result
    ///   starts a FRESH expression.
    /// * (Binary-op continuation is handled directly in [`press_op`]/
    ///   [`press_power`], which seed the buffer with the previous result.)
    /// * After an Error, any keypress starts fresh.
    fn begin_fresh_if_needed(&mut self) {
        if self.state != CalcState::Input {
            self.reset_input();
        }
    }

    /// Seed the buffer with the last result as a single number chunk. Used when
    /// a binary operator is pressed right after `=`.
    fn seed_with_last_result(&mut self) -> bool {
        if let Some((v, _)) = self.last_result {
            self.buf.clear();
            self.buf.push(Chunk::Number(format_seed(v)));
            self.state = CalcState::Input;
            self.error = None;
            true
        } else {
            false
        }
    }

    // ---- input -------------------------------------------------------------

    /// Append a digit `'0'..='9'`.
    pub fn press_digit(&mut self, c: char) {
        if !c.is_ascii_digit() {
            return;
        }
        self.begin_fresh_if_needed();
        match self.buf.last_mut() {
            Some(Chunk::Number(n)) => n.push(c),
            _ => self.buf.push(Chunk::Number(c.to_string())),
        }
    }

    /// Append a decimal point, starting a new number if needed and refusing a
    /// second dot in the same number.
    pub fn press_dot(&mut self) {
        self.begin_fresh_if_needed();
        match self.buf.last_mut() {
            Some(Chunk::Number(n)) => {
                if !n.contains('.') {
                    n.push('.');
                }
            }
            _ => self.buf.push(Chunk::Number("0.".to_string())),
        }
    }

    /// Append a binary operator. Right after `=`, this continues from the
    /// previous result. A leading `+ * / ^` on an empty buffer is dropped
    /// (nothing to operate on); a leading `-` is kept as unary minus.
    pub fn press_op(&mut self, op: Op) {
        if self.state == CalcState::Result {
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }

        // Leading operator on an empty buffer: only `-` (unary) is allowed.
        if self.buf.is_empty() {
            if op == Op::Sub {
                self.buf.push(Chunk::Sym("-"));
            }
            return;
        }

        // Replace a dangling trailing binary operator instead of stacking.
        if self.last_is_binary_op() {
            self.buf.pop();
        }
        self.buf.push(Chunk::Sym(op.canonical()));
    }

    /// Append `^` (power). Behaves like a binary operator for seeding/replacing.
    pub fn press_power(&mut self) {
        if self.state == CalcState::Result {
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }
        if self.buf.is_empty() {
            return; // nothing to raise
        }
        if self.last_is_binary_op() {
            self.buf.pop();
        }
        self.buf.push(Chunk::Sym("^"));
    }

    /// Smart parenthesis: insert `(` when opening a group is sensible, else `)`
    /// to close an outstanding one.
    ///
    /// Rule: insert `(` if the buffer is empty, or the last chunk is a binary
    /// operator / `(` / a function-open. Insert `)` if there is an unclosed `(`
    /// AND the last chunk ends a value (`number`, `)`, const, `%`, `!`).
    /// Otherwise default to `(`.
    pub fn press_paren(&mut self) {
        self.begin_fresh_if_needed();

        let want_open = self.buf.is_empty()
            || matches!(self.buf.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^" | "("))
            || matches!(self.buf.last(), Some(Chunk::Func(_)));

        if want_open {
            self.buf.push(Chunk::Sym("("));
            return;
        }

        if self.open_paren_count() > 0 && self.last_ends_value() {
            self.buf.push(Chunk::Sym(")"));
        } else {
            self.buf.push(Chunk::Sym("("));
        }
    }

    /// Append `%`.
    pub fn press_percent(&mut self) {
        // Percent applies to the current value; it only makes sense after one.
        if self.state == CalcState::Result {
            // Turn the result into a percent expression, e.g. 50 → 50%.
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }
        if self.last_ends_value() {
            self.buf.push(Chunk::Sym("%"));
        }
    }

    /// Append `!` (factorial). Only after a value.
    pub fn press_factorial(&mut self) {
        if self.state == CalcState::Result {
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }
        if self.last_ends_value() {
            self.buf.push(Chunk::Sym("!"));
        }
    }

    /// Append `√` — or, when [`inv`](Self::inv) is on, apply a *square* by
    /// appending `^2` to the current value.
    pub fn press_sqrt(&mut self) {
        if self.inv {
            // x²: square the current value/result.
            if self.state == CalcState::Result {
                self.seed_with_last_result();
            } else if self.state == CalcState::Error {
                self.reset_input();
            }
            if self.last_ends_value() {
                self.buf.push(Chunk::Sym("^"));
                self.buf.push(Chunk::Number("2".to_string()));
            }
            return;
        }
        self.begin_fresh_if_needed();
        self.buf.push(Chunk::Sym("√"));
    }

    /// Append the constant π.
    pub fn press_pi(&mut self) {
        self.begin_fresh_if_needed();
        self.buf.push(Chunk::Sym("π"));
    }

    /// Append the constant e.
    pub fn press_e(&mut self) {
        self.begin_fresh_if_needed();
        self.buf.push(Chunk::Sym("e"));
    }

    /// Append `abs(` (absolute value). Opens a paren; smart-paren/auto-close
    /// handles the rest.
    pub fn press_abs(&mut self) {
        self.begin_fresh_if_needed();
        self.buf.push(Chunk::Func("abs("));
    }

    /// Append `log2(` (base-2 logarithm). Opens a paren; auto-close handles the
    /// rest.
    pub fn press_log2(&mut self) {
        self.begin_fresh_if_needed();
        self.buf.push(Chunk::Func("log2("));
    }

    /// Reciprocal: append `^-1` to the current value (postfix), so `5` becomes
    /// `5^-1` = 0.2. Behaves like the `x²` inverse path of [`press_sqrt`].
    pub fn press_reciprocal(&mut self) {
        if self.state == CalcState::Result {
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }
        if self.last_ends_value() {
            self.buf.push(Chunk::Sym("^"));
            self.buf.push(Chunk::Sym("-"));
            self.buf.push(Chunk::Number("1".to_string()));
        }
    }

    /// Toggle the sign of the current trailing number operand. If the buffer
    /// ends in a `Number` preceded by a unary minus, remove that minus;
    /// otherwise insert a `-` before the number. When there is nothing to
    /// negate, insert a leading unary minus for the next number typed.
    pub fn press_negate(&mut self) {
        if self.state == CalcState::Result {
            self.seed_with_last_result();
        } else if self.state == CalcState::Error {
            self.reset_input();
        }
        let n = self.buf.len();
        if n == 0 {
            self.buf.push(Chunk::Sym("-"));
            return;
        }
        // Only act when the buffer ends in a Number (an operand to negate).
        if !matches!(self.buf.last(), Some(Chunk::Number(_))) {
            self.buf.push(Chunk::Sym("-"));
            return;
        }
        // The operand is the single trailing Number chunk at index n-1.
        let op_start = n - 1;
        // Is there a unary minus directly before it that we should toggle off?
        let has_unary_minus = op_start > 0
            && matches!(self.buf.get(op_start - 1), Some(Chunk::Sym("-")))
            && (op_start == 1
                || matches!(
                    self.buf.get(op_start - 2),
                    Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^" | "(")
                )
                || matches!(self.buf.get(op_start - 2), Some(Chunk::Func(_))));
        if has_unary_minus {
            self.buf.remove(op_start - 1);
        } else {
            self.buf.insert(op_start, Chunk::Sym("-"));
        }
    }

    /// Append a function-open (`sin(` …), including the hyperbolic trio
    /// (`sinh(`/`cosh(`/`tanh(`). With [`inv`](Self::inv) on, inserts the
    /// inverse form: Sin→`asin(`, Cos→`acos(`, Tan→`atan(`, Ln→`exp(`,
    /// Log→`10^` (a power of ten, not a function-open), and the hyperbolic
    /// inverses Sinh→`asinh(`, Cosh→`acosh(`, Tanh→`atanh(`.
    pub fn press_func(&mut self, f: Func) {
        self.begin_fresh_if_needed();
        if self.inv {
            match f {
                Func::Sin => self.buf.push(Chunk::Func("asin(")),
                Func::Cos => self.buf.push(Chunk::Func("acos(")),
                Func::Tan => self.buf.push(Chunk::Func("atan(")),
                Func::Ln => self.buf.push(Chunk::Func("exp(")),
                Func::Log => {
                    // 10ˣ: insert "10^". Stored as a number and a caret so it
                    // reads and backspaces naturally.
                    if self.last_ends_value() {
                        self.buf.push(Chunk::Sym("*"));
                    }
                    self.buf.push(Chunk::Number("10".to_string()));
                    self.buf.push(Chunk::Sym("^"));
                }
                Func::Sinh => self.buf.push(Chunk::Func("asinh(")),
                Func::Cosh => self.buf.push(Chunk::Func("acosh(")),
                Func::Tanh => self.buf.push(Chunk::Func("atanh(")),
            }
            return;
        }
        let name = match f {
            Func::Sin => "sin(",
            Func::Cos => "cos(",
            Func::Tan => "tan(",
            Func::Ln => "ln(",
            Func::Log => "log(",
            Func::Sinh => "sinh(",
            Func::Cosh => "cosh(",
            Func::Tanh => "tanh(",
        };
        self.buf.push(Chunk::Func(name));
    }

    /// Insert a previously-computed result value (as produced by
    /// [`engine::format_result`] and stored in a [`HistoryEntry::result`]) into
    /// the current expression as a numeric literal. Used by the history view to
    /// let a tapped entry seed the next calculation.
    ///
    /// The formatted result may be in E-notation (e.g. `"1.23E18"`, `"1E-9"`)
    /// for very large/small magnitudes, and grouped with commas. The engine's
    /// lexer accepts neither, so we normalize (strip grouping commas, map the
    /// Unicode minus `−` to ASCII `-`), parse to `f64` — which understands the
    /// exponent and sign — and re-emit the value via [`format_seed`] as a plain,
    /// ungrouped decimal literal the lexer round-trips. A negative value is
    /// stored as a leading unary-minus [`Chunk::Sym`] followed by the magnitude,
    /// mirroring how a typed unary minus is represented. Non-finite / unparseable
    /// inputs (e.g. `"∞"`, `"NaN"`) insert nothing rather than corrupt the buffer.
    ///
    /// After a `=` result or an error the buffer is started fresh (mirroring
    /// [`press_digit`](Self::press_digit)); a value following an existing value
    /// relies on the engine's implicit multiplication, matching
    /// [`memory_recall`](Self::memory_recall).
    ///
    /// [`HistoryEntry::result`]: crate::history::HistoryEntry::result
    pub fn insert_result(&mut self, value: &str) {
        self.begin_fresh_if_needed();
        // Normalize grouping and the Unicode minus, keeping any E-exponent and
        // its sign so `f64::parse` can read the full magnitude.
        let normalized: String = value
            .chars()
            .filter(|&c| c != ',')
            .map(|c| if c == '\u{2212}' { '-' } else { c })
            .collect();
        // A non-finite or otherwise unparseable value (e.g. "∞", "NaN") has no
        // valid literal form; leave the (freshly-begun) buffer untouched.
        // `f64::parse` accepts "NaN"/"inf"/"-inf", so guard `is_finite`
        // explicitly to keep an unlexable chunk (and a `format_seed` panic in
        // debug) out of the buffer.
        let Ok(v) = normalized.parse::<f64>() else {
            return;
        };
        if !v.is_finite() {
            return;
        }
        if self.last_ends_value() {
            self.buf.push(Chunk::Sym("*"));
        }
        // Re-emit as a plain decimal the lexer accepts, with a leading unary
        // minus for negatives (the lexer has no signed-literal grammar).
        if v.is_sign_negative() && v != 0.0 {
            self.buf.push(Chunk::Sym("-"));
            self.buf.push(Chunk::Number(format_seed(v.abs())));
        } else {
            self.buf.push(Chunk::Number(format_seed(v)));
        }
    }

    // ---- control -----------------------------------------------------------

    /// All-clear: empty the buffer and return to a clean [`CalcState::Input`].
    /// Memory and the angle unit are preserved.
    pub fn clear(&mut self) {
        self.reset_input();
        self.last_result = None;
        self.last_expr_display = None;
    }

    /// Delete the last *token*. A multi-digit number loses one digit; a function
    /// open (`sin(`) or a constant is removed whole. In a Result/Error state,
    /// backspace clears back to empty Input.
    pub fn backspace(&mut self) {
        if self.state != CalcState::Input {
            self.reset_input();
            return;
        }
        match self.buf.last_mut() {
            Some(Chunk::Number(n)) => {
                n.pop();
                if n.is_empty() {
                    self.buf.pop();
                }
            }
            Some(_) => {
                self.buf.pop();
            }
            None => {}
        }
    }

    /// Evaluate the current expression, commit it to history on success, and
    /// transition to [`CalcState::Result`] (or [`CalcState::Error`]).
    ///
    /// A trailing binary operator is stripped and missing parens auto-close
    /// (the engine does the latter). Returns the new [`HistoryEntry`] on
    /// success so the caller can persist it; returns `None` on error or when
    /// there is nothing to evaluate.
    pub fn equals(&mut self) -> Option<HistoryEntry> {
        if self.buf.is_empty() {
            return None;
        }

        // Work on a copy with any trailing binary operator stripped.
        let mut chunks = self.buf.clone();
        while matches!(chunks.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^")) {
            chunks.pop();
        }
        if chunks.is_empty() {
            return None;
        }

        let canonical = canonical_string(&chunks);
        let expr_display = pretty_string(&chunks);

        match engine::evaluate(&canonical, self.angle) {
            Ok(value) => {
                let result = engine::format_result(value);
                let entry = HistoryEntry::new(expr_display.clone(), result.clone());
                self.last_result = Some((value, result));
                self.last_expr_display = Some(expr_display);
                self.state = CalcState::Result;
                self.error = None;
                // The buffer is left holding the (stripped) expression so the
                // display can still show it alongside the result until the next
                // keypress replaces it.
                self.buf = chunks;
                Some(entry)
            }
            Err(e) => {
                self.state = CalcState::Error;
                self.error = Some(e.message().to_string());
                None
            }
        }
    }

    // ---- toggles -----------------------------------------------------------

    /// Flip the scientific inverse (2nd-function) mode.
    pub fn toggle_inv(&mut self) {
        self.inv = !self.inv;
    }

    /// Set the scientific inverse (2nd-function) mode directly.
    pub fn set_inv(&mut self, v: bool) {
        self.inv = v;
    }

    /// Whether inverse mode is on.
    pub fn inv(&self) -> bool {
        self.inv
    }

    /// Set the trig angle unit.
    pub fn set_angle(&mut self, a: AngleUnit) {
        self.angle = a;
    }

    /// The current trig angle unit.
    pub fn angle(&self) -> AngleUnit {
        self.angle
    }

    // ---- memory ------------------------------------------------------------

    /// MS: store the current value (result or live eval) in memory.
    pub fn memory_store(&mut self) {
        if let Some(v) = self.current_value() {
            self.memory = Some(v);
        }
    }

    /// MR: insert the stored value, gluing to a preceding value with implicit
    /// multiplication and representing a negative as a leading unary minus.
    pub fn memory_recall(&mut self) {
        if let Some(v) = self.memory {
            self.begin_fresh_if_needed();
            if self.last_ends_value() {
                self.buf.push(Chunk::Sym("*"));
            }
            if v.is_sign_negative() && v != 0.0 {
                self.buf.push(Chunk::Sym("-"));
                self.buf.push(Chunk::Number(format_seed(v.abs())));
            } else {
                self.buf.push(Chunk::Number(format_seed(v)));
            }
        }
    }

    /// M+: add the current value to memory (initialising it to 0 if empty). A
    /// non-finite (overflow) result leaves the register unchanged.
    pub fn memory_add(&mut self) {
        if let Some(v) = self.current_value() {
            let candidate = self.memory.unwrap_or(0.0) + v;
            if candidate.is_finite() {
                self.memory = Some(candidate);
            }
        }
    }

    /// M−: subtract the current value from memory. A non-finite (overflow)
    /// result leaves the register unchanged.
    pub fn memory_sub(&mut self) {
        if let Some(v) = self.current_value() {
            let candidate = self.memory.unwrap_or(0.0) - v;
            if candidate.is_finite() {
                self.memory = Some(candidate);
            }
        }
    }

    /// MC: clear the memory register.
    pub fn memory_clear(&mut self) {
        self.memory = None;
    }

    /// Whether a value is stored in memory.
    pub fn has_memory(&self) -> bool {
        self.memory.is_some()
    }

    // ---- readout -----------------------------------------------------------

    /// The buffer pretty-printed for the display using en-US separators
    /// (group ',', decimal '.'). Kept for callers/tests that don't thread a
    /// locale.
    pub fn display_expression(&self) -> String {
        self.display_expression_with(',', '.')
    }

    /// The buffer pretty-printed for display with explicit group + decimal
    /// separators. The canonical buffer is untouched (still ASCII '.'/digits);
    /// only the returned display string swaps separators.
    pub fn display_expression_with(&self, group: char, decimal: char) -> String {
        pretty_string_with(&self.buf, group, decimal)
    }

    /// The instant ("live") result, or `None`.
    ///
    /// Returns `None` when the expression is empty, is a bare number, has no
    /// "interesting" operation after stripping a trailing binary operator and an
    /// optional leading unary minus, or fails to evaluate for any reason. Never
    /// returns an error string — syntax and math errors alike become `None` to
    /// avoid preview flicker.
    pub fn live_result(&self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }

        // Strip trailing binary operators.
        let mut chunks = self.buf.clone();
        while matches!(chunks.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^")) {
            chunks.pop();
        }
        if chunks.is_empty() {
            return None;
        }

        if !has_interesting_op(&chunks) {
            return None;
        }

        let canonical = canonical_string(&chunks);
        let value = engine::evaluate(&canonical, self.angle).ok()?;
        Some(engine::format_result(value))
    }

    /// The live ("instant") result as a raw f64, applying the same
    /// "interesting op" gate as [`live_result`]. `None` when there is no
    /// meaningful preview. Lets the UI format with a locale.
    pub fn live_value(&self) -> Option<f64> {
        if self.buf.is_empty() {
            return None;
        }
        let mut chunks = self.buf.clone();
        while matches!(chunks.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^")) {
            chunks.pop();
        }
        if chunks.is_empty() {
            return None;
        }
        if !has_interesting_op(&chunks) {
            return None;
        }
        let canonical = canonical_string(&chunks);
        engine::evaluate(&canonical, self.angle).ok()
    }

    /// The current display mode.
    pub fn state(&self) -> CalcState {
        self.state
    }

    /// The error message when in [`CalcState::Error`], else `None`.
    pub fn error_message(&self) -> Option<String> {
        self.error.clone()
    }

    /// The numeric value of the current result or live evaluation, for memory
    /// operations and copy-to-clipboard. `None` if nothing evaluates.
    pub fn current_value(&self) -> Option<f64> {
        if let Some((v, _)) = self.last_result {
            if self.state == CalcState::Result {
                return Some(v);
            }
        }
        if self.buf.is_empty() {
            return None;
        }
        let mut chunks = self.buf.clone();
        while matches!(chunks.last(), Some(Chunk::Sym(s)) if matches!(*s, "+" | "-" | "*" | "/" | "^")) {
            chunks.pop();
        }
        if chunks.is_empty() {
            return None;
        }
        let canonical = canonical_string(&chunks);
        engine::evaluate(&canonical, self.angle).ok()
    }

    // ---- internal ----------------------------------------------------------

    /// Count unclosed `(` in the buffer (opens minus closes, floored at 0).
    fn open_paren_count(&self) -> i32 {
        let mut depth: i32 = 0;
        for c in &self.buf {
            match c {
                Chunk::Sym("(") => depth += 1,
                Chunk::Sym(")") => depth = (depth - 1).max(0),
                Chunk::Func(_) => depth += 1, // a func-open contributes an open paren
                _ => {}
            }
        }
        depth
    }
}

// ---- free helpers ----------------------------------------------------------

/// Format a value for reuse as a seed/number chunk. Uses a plain (ungrouped)
/// representation so it re-tokenizes cleanly. Emits the shortest round-tripping
/// decimal (no fixed precision, no trailing-zero trimming needed), so tiny/huge
/// magnitudes are lossless. Integers take a fast path.
fn format_seed(v: f64) -> String {
    debug_assert!(v.is_finite());
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // Shortest round-tripping decimal. `{}` never emits exponent notation
        // for f64, so this is always a plain decimal the lexer accepts, and it
        // carries no trailing-zero cruft to trim.
        format!("{v}")
    }
}

/// Build the canonical (engine-facing) string from chunks.
fn canonical_string(chunks: &[Chunk]) -> String {
    let mut s = String::new();
    for c in chunks {
        match c {
            Chunk::Number(n) => s.push_str(n),
            Chunk::Sym(sym) => s.push_str(sym),
            Chunk::Func(f) => s.push_str(f),
        }
    }
    s
}

/// Build the pretty (display) string from chunks: ASCII operators become their
/// Unicode display glyphs and numeric literals are thousands-grouped.
///
/// Thin en-US wrapper (group ',', decimal '.') kept for `equals()` history
/// entries and other callers that don't thread a locale.
fn pretty_string(chunks: &[Chunk]) -> String {
    pretty_string_with(chunks, ',', '.')
}

/// Build the pretty display string with explicit group + decimal separators.
/// The canonical buffer is untouched; only this output localizes separators.
fn pretty_string_with(chunks: &[Chunk], group: char, decimal: char) -> String {
    let mut s = String::new();
    for c in chunks {
        match c {
            Chunk::Number(n) => s.push_str(&group_number_literal_with(n, group, decimal)),
            Chunk::Sym(sym) => s.push_str(pretty_sym(sym)),
            Chunk::Func(f) => s.push_str(f),
        }
    }
    s
}

/// Map a canonical single-glyph symbol to its display form.
fn pretty_sym(sym: &str) -> &str {
    match sym {
        "*" => "×",
        "/" => "÷",
        "-" => "\u{2212}", // U+2212 MINUS SIGN
        other => other,     // + ^ ! % ( ) √ π e pass through
    }
}

/// Group the integer part of a numeric literal with `group`, and render the
/// decimal point (and any fractional part) using `decimal`. The input `n` is
/// the CANONICAL literal (ASCII '.'); only the output is localized.
fn group_number_literal_with(n: &str, group: char, decimal: char) -> String {
    let (int_part, frac_with_dot) = match n.find('.') {
        Some(dot) => (&n[..dot], &n[dot..]), // frac_with_dot starts with '.'
        None => (n, ""),
    };
    // Localize the fractional part's leading '.' to `decimal`.
    let frac_localized = if frac_with_dot.is_empty() {
        String::new()
    } else {
        let mut f = String::new();
        f.push(decimal);
        f.push_str(&frac_with_dot[1..]); // everything after the '.'
        f
    };
    if int_part.len() <= 3 {
        return format!("{}{}", int_part, frac_localized);
    }
    let len = int_part.len();
    let mut grouped = String::with_capacity(len + len / 3 + frac_localized.len());
    for (idx, ch) in int_part.chars().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(ch);
    }
    grouped.push_str(&frac_localized);
    grouped
}

/// The "interesting ops" gate for [`Calculator::live_result`]: after stripping a
/// leading unary minus, the chunk list must contain at least one operator or
/// function (so a bare number — even negative — yields no live preview).
fn has_interesting_op(chunks: &[Chunk]) -> bool {
    let start = if matches!(chunks.first(), Some(Chunk::Sym("-"))) {
        1
    } else {
        0
    };
    chunks[start..].iter().any(|c| match c {
        Chunk::Sym(s) => matches!(
            *s,
            "+" | "-" | "*" | "/" | "^" | "√" | "%" | "!"
        ),
        Chunk::Func(_) => true,
        Chunk::Number(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> Calculator {
        Calculator::new(AngleUnit::Rad)
    }

    fn type_str(calc: &mut Calculator, s: &str) {
        for ch in s.chars() {
            match ch {
                '0'..='9' => calc.press_digit(ch),
                '.' => calc.press_dot(),
                '+' => calc.press_op(Op::Add),
                '-' => calc.press_op(Op::Sub),
                '*' => calc.press_op(Op::Mul),
                '/' => calc.press_op(Op::Div),
                '^' => calc.press_power(),
                _ => panic!("unsupported test char {ch}"),
            }
        }
    }

    // ---- live_result -------------------------------------------------------

    #[test]
    fn live_bare_number_is_none() {
        let mut calc = c();
        type_str(&mut calc, "5");
        assert_eq!(calc.live_result(), None);
    }

    #[test]
    fn live_simple_sum() {
        let mut calc = c();
        type_str(&mut calc, "5+3");
        assert_eq!(calc.live_result(), Some("8".to_string()));
    }

    #[test]
    fn live_trailing_op_is_none() {
        let mut calc = c();
        type_str(&mut calc, "5+");
        assert_eq!(calc.live_result(), None);
    }

    #[test]
    fn live_negative_number_is_none() {
        let mut calc = c();
        type_str(&mut calc, "-5");
        assert_eq!(calc.live_result(), None);
    }

    #[test]
    fn live_swallows_divide_by_zero() {
        let mut calc = c();
        type_str(&mut calc, "5/0");
        assert_eq!(calc.live_result(), None);
    }

    // ---- display -----------------------------------------------------------

    #[test]
    fn display_uses_pretty_glyphs() {
        let mut calc = c();
        type_str(&mut calc, "5*8");
        assert_eq!(calc.display_expression(), "5×8");
    }

    #[test]
    fn display_minus_and_divide() {
        let mut calc = c();
        type_str(&mut calc, "9-3/2");
        assert_eq!(calc.display_expression(), "9\u{2212}3÷2");
    }

    #[test]
    fn display_groups_number_literals() {
        let mut calc = c();
        type_str(&mut calc, "2024+1");
        assert_eq!(calc.display_expression(), "2,024+1");
    }

    // ---- equals + result seeding ------------------------------------------

    #[test]
    fn equals_produces_history_entry() {
        let mut calc = c();
        type_str(&mut calc, "2+3");
        let entry = calc.equals().unwrap();
        assert_eq!(entry.result, "5");
        assert_eq!(calc.state(), CalcState::Result);
    }

    #[test]
    fn equals_then_op_continues_from_result() {
        let mut calc = c();
        type_str(&mut calc, "2+3");
        calc.equals();
        calc.press_op(Op::Mul);
        calc.press_digit('4');
        // (2+3)=5, then ×4 = 20.
        assert_eq!(calc.live_result(), Some("20".to_string()));
    }

    #[test]
    fn equals_then_digit_starts_fresh() {
        let mut calc = c();
        type_str(&mut calc, "2+3");
        calc.equals();
        calc.press_digit('7');
        assert_eq!(calc.display_expression(), "7");
        assert_eq!(calc.state(), CalcState::Input);
    }

    #[test]
    fn equals_strips_trailing_operator() {
        let mut calc = c();
        type_str(&mut calc, "5+");
        let entry = calc.equals().unwrap();
        assert_eq!(entry.result, "5");
    }

    #[test]
    fn equals_error_sets_message() {
        let mut calc = c();
        type_str(&mut calc, "5/0");
        assert!(calc.equals().is_none());
        assert_eq!(calc.state(), CalcState::Error);
        assert_eq!(calc.error_message().as_deref(), Some("Can't divide by 0"));
    }

    // ---- backspace / clear -------------------------------------------------

    #[test]
    fn backspace_deletes_one_digit() {
        let mut calc = c();
        type_str(&mut calc, "123");
        calc.backspace();
        assert_eq!(calc.display_expression(), "12");
    }

    #[test]
    fn backspace_deletes_function_as_unit() {
        let mut calc = c();
        calc.press_func(Func::Sin); // "sin("
        assert_eq!(calc.display_expression(), "sin(");
        calc.backspace();
        assert_eq!(calc.display_expression(), "");
    }

    #[test]
    fn clear_resets() {
        let mut calc = c();
        type_str(&mut calc, "1+2");
        calc.clear();
        assert_eq!(calc.display_expression(), "");
        assert_eq!(calc.state(), CalcState::Input);
    }

    // ---- inverse mode ------------------------------------------------------

    #[test]
    fn inv_func_inserts_inverse() {
        let mut calc = c();
        calc.toggle_inv();
        calc.press_func(Func::Sin);
        assert_eq!(calc.display_expression(), "asin(");
    }

    #[test]
    fn inv_asin_one_deg_is_ninety() {
        let mut calc = Calculator::new(AngleUnit::Deg);
        calc.toggle_inv();
        calc.press_func(Func::Sin); // asin(
        calc.press_digit('1');
        // asin(1) with auto-close in Deg = 90.
        assert_eq!(calc.live_result(), Some("90".to_string()));
    }

    #[test]
    fn set_inv_sets_flag() {
        let mut calc = c();
        calc.set_inv(true);
        assert!(calc.inv());
        calc.set_inv(false);
        assert!(!calc.inv());
    }

    #[test]
    fn toggle_inv_twice_returns_to_normal() {
        let mut calc = c();
        calc.toggle_inv();
        calc.toggle_inv();
        assert!(!calc.inv());
        calc.press_func(Func::Sin);
        assert_eq!(calc.display_expression(), "sin(");
    }

    #[test]
    fn inv_sqrt_is_square() {
        let mut calc = c();
        type_str(&mut calc, "3");
        calc.toggle_inv();
        calc.press_sqrt(); // x²  → appends ^2
        assert_eq!(calc.live_result(), Some("9".to_string()));
    }

    #[test]
    fn inv_log_is_power_of_ten() {
        let mut calc = c();
        calc.toggle_inv();
        calc.press_func(Func::Log); // 10^
        calc.press_digit('3');
        assert_eq!(calc.live_result(), Some("1,000".to_string()));
    }

    // ---- smart paren -------------------------------------------------------

    #[test]
    fn paren_on_empty_opens() {
        let mut calc = c();
        calc.press_paren();
        assert_eq!(calc.display_expression(), "(");
    }

    #[test]
    fn paren_after_value_with_open_closes() {
        let mut calc = c();
        calc.press_paren(); // (
        calc.press_digit('5'); // (5
        calc.press_paren(); // should close → (5)
        assert_eq!(calc.display_expression(), "(5)");
    }

    #[test]
    fn paren_after_operator_opens() {
        let mut calc = c();
        calc.press_paren(); // (
        calc.press_digit('5');
        calc.press_op(Op::Add); // (5+
        calc.press_paren(); // last is operator → open → (5+(
        assert_eq!(calc.display_expression(), "(5+(");
    }

    #[test]
    fn paren_after_value_no_open_opens_with_implicit_mult() {
        let mut calc = c();
        calc.press_digit('5');
        calc.press_paren(); // no open paren, last is value → default "(" → "5("
        assert_eq!(calc.display_expression(), "5(");
    }

    // ---- memory ------------------------------------------------------------

    #[test]
    fn memory_store_recall() {
        let mut calc = c();
        type_str(&mut calc, "2+3");
        calc.equals(); // result 5
        assert!(!calc.has_memory());
        calc.memory_store();
        assert!(calc.has_memory());
        calc.clear();
        calc.memory_recall();
        assert_eq!(calc.display_expression(), "5");
    }

    #[test]
    fn memory_add_sub() {
        let mut calc = c();
        type_str(&mut calc, "10");
        calc.equals();
        calc.memory_store(); // M = 10
        calc.clear();
        type_str(&mut calc, "4");
        calc.equals();
        calc.memory_add(); // M = 14
        calc.clear();
        type_str(&mut calc, "1");
        calc.equals();
        calc.memory_sub(); // M = 13
        calc.clear();
        calc.memory_recall();
        assert_eq!(calc.display_expression(), "13");
    }

    #[test]
    fn memory_clear_works() {
        let mut calc = c();
        type_str(&mut calc, "5");
        calc.equals();
        calc.memory_store();
        assert!(calc.has_memory());
        calc.memory_clear();
        assert!(!calc.has_memory());
    }

    // ---- current_value -----------------------------------------------------

    #[test]
    fn current_value_of_result() {
        let mut calc = c();
        type_str(&mut calc, "6*7");
        calc.equals();
        assert_eq!(calc.current_value(), Some(42.0));
    }

    // ---- angle -------------------------------------------------------------

    #[test]
    fn set_and_read_angle() {
        let mut calc = c();
        assert_eq!(calc.angle(), AngleUnit::Rad);
        calc.set_angle(AngleUnit::Deg);
        assert_eq!(calc.angle(), AngleUnit::Deg);
    }

    // ---- double dot / dot start -------------------------------------------

    #[test]
    fn dot_starts_zero_point() {
        let mut calc = c();
        calc.press_dot();
        assert_eq!(calc.display_expression(), "0.");
        calc.press_digit('5');
        assert_eq!(calc.display_expression(), "0.5");
    }

    #[test]
    fn second_dot_ignored() {
        let mut calc = c();
        type_str(&mut calc, "3.5");
        calc.press_dot();
        assert_eq!(calc.display_expression(), "3.5");
    }

    // ---- insert_result (history tap) --------------------------------------

    #[test]
    fn insert_result_strips_grouping() {
        let mut calc = c();
        calc.insert_result("1,234.5");
        assert_eq!(calc.display_expression(), "1,234.5");
        assert_eq!(calc.state(), CalcState::Input);
    }

    #[test]
    fn insert_result_keeps_leading_sign() {
        let mut calc = c();
        calc.insert_result("\u{2212}42");
        assert_eq!(calc.display_expression(), "\u{2212}42");
    }

    #[test]
    fn insert_result_after_equals_starts_fresh() {
        let mut calc = c();
        type_str(&mut calc, "2+3");
        calc.equals();
        calc.insert_result("10");
        assert_eq!(calc.display_expression(), "10");
        assert_eq!(calc.state(), CalcState::Input);
    }

    #[test]
    fn insert_result_expands_scientific_large() {
        let mut calc = c();
        calc.insert_result("1.23E18");
        let v = calc.current_value().expect("bare number should evaluate");
        assert!(
            (v / 1.23e18 - 1.0).abs() < 1e-9,
            "expected ~1.23e18, got {v}"
        );
    }

    #[test]
    fn insert_result_expands_scientific_small() {
        let mut calc = c();
        calc.insert_result("1E-9");
        let v = calc.current_value().expect("bare number should evaluate");
        assert!(
            (v / 1e-9 - 1.0).abs() < 1e-6,
            "expected ~1e-9, got {v}"
        );
    }

    #[test]
    fn insert_result_strips_grouping_value() {
        let mut calc = c();
        calc.insert_result("1,000,000");
        let v = calc.current_value().expect("bare number should evaluate");
        assert!((v - 1e6).abs() < 1.0, "expected ~1e6, got {v}");
    }

    #[test]
    fn insert_result_negative_value_roundtrips() {
        let mut calc = c();
        calc.insert_result("-625");
        let v = calc.current_value().expect("bare number should evaluate");
        assert!((v - (-625.0)).abs() < 1e-9, "expected -625, got {v}");
    }

    #[test]
    fn insert_result_ignores_nan() {
        let mut calc = c();
        calc.insert_result("NaN"); // must not panic
        assert_eq!(calc.current_value(), None);
    }

    #[test]
    fn insert_result_ignores_inf() {
        let mut calc = c();
        calc.insert_result("inf"); // must not panic
        assert_eq!(calc.current_value(), None);
    }

    #[test]
    fn insert_result_accepts_finite() {
        let mut calc = c();
        calc.insert_result("42");
        let v = calc.current_value().expect("bare number should evaluate");
        assert!((v - 42.0).abs() < 1e-9, "expected 42, got {v}");
    }

    // ---- hyperbolic + new sci functions -----------------------------------

    #[test]
    fn func_sinh_display() {
        let mut calc = c();
        calc.press_func(Func::Sinh);
        assert_eq!(calc.display_expression(), "sinh(");
    }

    #[test]
    fn inv_func_sinh_is_asinh() {
        let mut calc = c();
        calc.toggle_inv();
        calc.press_func(Func::Sinh);
        assert_eq!(calc.display_expression(), "asinh(");
    }

    #[test]
    fn abs_of_negative() {
        let mut calc = c();
        calc.press_abs(); // "abs("
        type_str(&mut calc, "-5"); // abs(-5  → auto-closes to abs(-5)
        assert_eq!(calc.live_result(), Some("5".to_string()));
    }

    #[test]
    fn log2_of_eight() {
        let mut calc = c();
        calc.press_log2(); // "log2("
        calc.press_digit('8'); // log2(8 → auto-closes
        assert_eq!(calc.live_result(), Some("3".to_string()));
    }

    #[test]
    fn reciprocal_of_four() {
        let mut calc = c();
        type_str(&mut calc, "4");
        calc.press_reciprocal(); // 4^-1
        assert_eq!(calc.live_result(), Some("0.25".to_string()));
    }

    #[test]
    fn negate_toggles_trailing_number() {
        let mut calc = c();
        type_str(&mut calc, "5");
        calc.press_negate();
        assert_eq!(calc.display_expression(), "\u{2212}5");
        calc.press_negate();
        assert_eq!(calc.display_expression(), "5");
    }

    #[test]
    fn memory_recall_after_value_multiplies() {
        let mut calc = c();
        type_str(&mut calc, "5");
        calc.equals();
        calc.memory_store();
        calc.clear();
        type_str(&mut calc, "2");
        calc.memory_recall();
        assert_eq!(calc.current_value(), Some(10.0)); // 2*5, not 25
    }

    #[test]
    fn memory_recall_negative_is_multiply_not_subtract() {
        let mut calc = c();
        type_str(&mut calc, "-5");
        calc.equals();
        calc.memory_store();
        calc.clear();
        type_str(&mut calc, "2");
        calc.memory_recall();
        assert_eq!(calc.current_value(), Some(-10.0)); // 2*-5, not -3
    }

    #[test]
    fn insert_result_after_value_multiplies() {
        let mut calc = c();
        type_str(&mut calc, "2");
        calc.insert_result("5");
        assert_eq!(calc.current_value(), Some(10.0));
    }

    #[test]
    fn insert_result_negative_after_value() {
        let mut calc = c();
        type_str(&mut calc, "2");
        calc.insert_result("-5");
        assert_eq!(calc.current_value(), Some(-10.0)); // 2 * -5
    }

    #[test]
    fn inverse_log_after_value_multiplies() {
        let mut calc = c();
        type_str(&mut calc, "2");
        calc.toggle_inv();
        calc.press_func(Func::Log);
        calc.set_inv(false);
        calc.press_digit('3');
        assert_eq!(calc.current_value(), Some(2000.0)); // 2*10^3
    }

    #[test]
    fn tiny_result_seeds_losslessly() {
        let s = format_seed(1e-13);
        let v = engine::evaluate(&s, AngleUnit::Rad).unwrap();
        assert!((v / 1e-13 - 1.0).abs() < 1e-9, "got {v} from {s}");
    }

    #[test]
    fn format_seed_round_trips() {
        for v in [3.14, 1e-13, 1e20, 1.5e-8, 0.000001, 123456.789] {
            let s = format_seed(v);
            let got = engine::evaluate(&s, AngleUnit::Rad).unwrap();
            assert!((got / v - 1.0).abs() < 1e-9, "v={v} s={s} got={got}");
        }
    }

    #[test]
    fn memory_add_overflow_rejected() {
        let mut calc = c();
        calc.insert_result("1e308");
        calc.equals();
        calc.memory_store(); // M = 1e308
        calc.clear();
        calc.insert_result("1e308");
        calc.equals();
        calc.memory_add(); // candidate = 2e308 = inf -> rejected
        assert!(calc.has_memory());
        calc.clear();
        calc.memory_recall();
        let v = calc.current_value().unwrap();
        assert!(v.is_finite(), "memory must stay finite, got {v}");
        assert!((v / 1e308 - 1.0).abs() < 1e-9, "memory should remain 1e308, got {v}");
    }
}
