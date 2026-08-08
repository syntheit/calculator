//! The pure-Rust evaluation engine.
//!
//! The engine is UI-agnostic: it turns a *canonical* expression string into a
//! number (or a typed error) and formats numbers for display. The pipeline is
//!
//! ```text
//! &str ──lexer::tokenize──▶ Vec<Token> ──parser::parse──▶ Expr ──eval::eval──▶ f64
//! ```
//!
//! plus [`format::format_result`] for rendering. The [`crate::state`] machine
//! bridges pretty display glyphs (`× ÷ −`) to this canonical form and drives the
//! engine on every keypress for the live preview.
//!
//! Public surface (re-exported below):
//! * [`AngleUnit`] — radians / degrees.
//! * [`EvalError`] — typed failure with AOSP-style display messages.
//! * [`evaluate`] — canonical string → `f64`.
//! * [`format_result`] — `f64` → display string.
//! * lower-level [`lexer`], [`parser`], [`eval`], [`format`] modules for tests
//!   and the state machine.

pub mod eval;
pub mod format;
pub mod lexer;
pub mod parser;

pub use eval::AngleUnit;
pub use format::format_result;

/// A typed evaluation failure. Each variant maps to the AOSP ExactCalculator
/// display message via [`EvalError::message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    /// The expression could not be parsed (bad token, trailing operator, …).
    #[error("Bad expression")]
    Syntax,
    /// Division by zero.
    #[error("Can't divide by 0")]
    DivideByZero,
    /// The result is infinite (overflow).
    #[error("Infinite?")]
    Overflow,
    /// The result is NaN.
    #[error("Not a number")]
    NotANumber,
    /// The operation is outside its domain (√ of a negative, ln of ≤0, …).
    #[error("Not a number")]
    Domain,
}

impl EvalError {
    /// The user-facing message shown in the display, matching AOSP.
    pub fn message(self) -> &'static str {
        match self {
            EvalError::Syntax => "Bad expression",
            EvalError::DivideByZero => "Can't divide by 0",
            EvalError::Overflow => "Infinite?",
            EvalError::NotANumber => "Not a number",
            EvalError::Domain => "Not a number",
        }
    }
}

/// Evaluate a canonical expression string end-to-end.
///
/// This is the one call the state machine needs: it tokenizes (inserting
/// implicit multiplication), parses (auto-closing missing parens), and
/// evaluates under `angle`.
pub fn evaluate(input: &str, angle: AngleUnit) -> Result<f64, EvalError> {
    let tokens = lexer::tokenize(input)?;
    let ast = parser::parse(&tokens)?;
    eval::eval(&ast, angle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn end_to_end_percent() {
        assert!(close(evaluate("100+10%", AngleUnit::Rad).unwrap(), 110.0));
    }

    #[test]
    fn end_to_end_error_message() {
        assert_eq!(
            evaluate("5/0", AngleUnit::Rad).unwrap_err().message(),
            "Can't divide by 0"
        );
        assert_eq!(
            evaluate("5+", AngleUnit::Rad).unwrap_err().message(),
            "Bad expression"
        );
    }

    #[test]
    fn end_to_end_auto_close() {
        assert!(close(evaluate("cos(\u{03C0}", AngleUnit::Rad).unwrap(), -1.0));
        assert!(close(evaluate("2*(3+4", AngleUnit::Rad).unwrap(), 14.0));
    }
}
