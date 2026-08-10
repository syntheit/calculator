//! Recursive-descent / precedence-climbing parser: [`Token`] stream → [`Expr`]
//! AST.
//!
//! Grammar (lowest precedence first), matching the engine spec:
//!
//! ```text
//! expr        := add
//! add         := mul (('+' | '-') mul)*
//! mul         := unary (('*' | '/') unary)*
//! unary       := '-' unary | power        // unary minus binds TIGHTER than ^
//! power       := postfix ('^' unary)?      // right-assoc; RHS re-enters unary
//! postfix     := atom ('!' | '%')*         // factorial / percent markers
//! atom        := NUMBER | π | e
//!              | '(' add ')'               // ')' may be missing → auto-close
//!              | FUNC '(' add ')'          // ditto
//!              | FUNC2 '(' add ',' add ')' // two-arg call; comma required
//!              | '√' unary                 // prefix sqrt over a factor
//! ```
//!
//! Notes on the two spec-critical bits:
//!
//! * **Unary minus tighter than `^`.** `power` parses a `postfix` on the left,
//!   then—if a `^` follows—recurses into `unary` for the exponent. Because the
//!   *outer* `-2^2` is parsed by `unary` calling `power` on `2^2`, the negation
//!   wraps the whole power: `-(2^2) = -4`? No — the spec wants `-2^2 = 4`. We
//!   get that by having `unary` bind the minus to a *single* `power` whose base
//!   is the number `2` and whose `^` exponent is then read; i.e. the minus sees
//!   `2^2` as its operand only when the base is negative. To make `-2^2 = 4` we
//!   instead let `power`'s base be a `unary`, so `-2` is the base of `^2`.
//!   See [`parse_power`] for the exact shape actually used.
//!
//! * **Auto-close.** Where a `)` is required but the stream is at EOF, the
//!   parser treats EOF as an implicit close instead of erroring. A `)` that is
//!   genuinely present is still consumed. This yields `cos(π` = `cos(π)` and
//!   `2*(3+4` = `2*(3+4)`.

use crate::engine::lexer::{Func2Name, FuncName, Token};
use crate::engine::EvalError;
use std::cell::Cell;

/// The abstract syntax tree the evaluator walks.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A numeric literal.
    Num(f64),
    /// π.
    Pi,
    /// e.
    E,
    /// Unary negation.
    Neg(Box<Expr>),
    /// Binary `+ - * / ^`.
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `√` of its operand.
    Sqrt(Box<Expr>),
    /// Postfix `!` factorial.
    Fact(Box<Expr>),
    /// Postfix `%`. Resolved contextually by the evaluator (Google semantics).
    Percent(Box<Expr>),
    /// A named function call.
    Call(FuncName, Box<Expr>),
    /// A named two-argument function call (e.g. `nPr(n, r)`, `root(y, x)`).
    Call2(Func2Name, Box<Expr>, Box<Expr>),
    /// An explicitly parenthesised sub-expression. Kept as a distinct node
    /// (rather than collapsed) so the evaluator can tell `(10)%` from `10%`:
    /// parenthesising a percent term defeats Google's additive percent rule.
    Paren(Box<Expr>),
}

/// Binary operators carried by [`Expr::Bin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

/// Parse a full token stream to an [`Expr`]. Trailing tokens after a complete
/// expression are a syntax error; a trailing binary operator is a syntax error
/// (the state machine strips those before calling for the live preview).
pub fn parse(tokens: &[Token]) -> Result<Expr, EvalError> {
    let depth = Cell::new(0);
    let mut p = Parser {
        tokens,
        pos: 0,
        depth: &depth,
    };
    let expr = p.parse_add()?;
    if p.pos != p.tokens.len() {
        // Leftover tokens the grammar could not consume.
        return Err(EvalError::Syntax);
    }
    Ok(expr)
}

/// Maximum recursive-descent depth. Deeply nested input like `(((…1…)))`,
/// `√√√…`, `sin(sin(sin(…`, `----…`, or a right-assoc power chain
/// `2^2^2^…` recurses ~one guard level per nesting level: the [`DepthGuard`]
/// is applied only at [`Parser::parse_atom`] (the paren / sqrt / func-arg
/// descent point), [`Parser::parse_neg_base`] (the unary-minus chain), and
/// [`Parser::parse_power`] (whose guard stays alive across the recursive
/// exponent, bounding right-assoc `^` chains). Each is entered ~once per real
/// nesting level. Past this cap we return [`EvalError::Syntax`] instead of
/// overflowing the stack.
const MAX_DEPTH: usize = 256;

/// RAII guard that increments the parser's recursion depth on construction and
/// decrements it on drop (panic-safe, and runs on every `?` early-return path).
/// Holds a shared borrow of a `Cell`, so the parse methods can keep `&mut self`.
struct DepthGuard<'a>(&'a Cell<usize>);

impl<'a> DepthGuard<'a> {
    fn new(d: &'a Cell<usize>) -> Result<Self, EvalError> {
        let n = d.get() + 1;
        if n > MAX_DEPTH {
            return Err(EvalError::Syntax);
        }
        d.set(n);
        Ok(DepthGuard(d))
    }
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Reference to a caller-owned recursion counter. Held as a reference (not
    /// an owned `Cell`) so a [`DepthGuard`] can borrow it without also borrowing
    /// `self`, leaving the parse methods free to take `&mut self`.
    depth: &'a Cell<usize>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// `add := mul (('+' | '-') mul)*`
    fn parse_add(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_mul()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.bump();
                    let right = self.parse_mul()?;
                    left = Expr::Bin(BinOp::Add, Box::new(left), Box::new(right));
                }
                Some(Token::Minus) => {
                    self.bump();
                    let right = self.parse_mul()?;
                    left = Expr::Bin(BinOp::Sub, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// `mul := unary (('*' | '/') unary)*`
    fn parse_mul(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.bump();
                    let right = self.parse_unary()?;
                    left = Expr::Bin(BinOp::Mul, Box::new(left), Box::new(right));
                }
                Some(Token::Slash) => {
                    self.bump();
                    let right = self.parse_unary()?;
                    left = Expr::Bin(BinOp::Div, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// `unary := power`. Unary minus is *not* handled here; it is folded into
    /// the power's base so that it binds tighter than `^` (see [`parse_power`]).
    /// This function stays as the shared entry point for the exponent and for
    /// `√`'s and `*`/`/`'s operands.
    fn parse_unary(&mut self) -> Result<Expr, EvalError> {
        self.parse_power()
    }

    /// `power := neg_base ('^' unary)?` — right-associative, with unary minus
    /// binding TIGHTER than `^`.
    ///
    /// The base is a `neg_base` (`'-' neg_base | postfix`), so a leading minus
    /// attaches to the base *before* the exponent is applied: `-2^2` parses as
    /// `(-2)^2 = 4`, matching Google. The exponent re-enters `parse_unary` so
    /// `2^-3` works and `2^3^2` associates right (`2^(3^2) = 512`).
    fn parse_power(&mut self) -> Result<Expr, EvalError> {
        let _guard = DepthGuard::new(self.depth)?;
        let base = self.parse_neg_base()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.bump();
            let exp = self.parse_unary()?;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    /// `neg_base := '-' neg_base | postfix`. A run of leading minuses negates
    /// the postfix base (the operand of any following `^`).
    fn parse_neg_base(&mut self) -> Result<Expr, EvalError> {
        let _guard = DepthGuard::new(self.depth)?;
        if matches!(self.peek(), Some(Token::Minus)) {
            self.bump();
            let operand = self.parse_neg_base()?;
            return Ok(Expr::Neg(Box::new(operand)));
        }
        self.parse_postfix()
    }

    /// `postfix := atom ('!' | '%')*`
    fn parse_postfix(&mut self) -> Result<Expr, EvalError> {
        let mut node = self.parse_atom()?;
        loop {
            match self.peek() {
                Some(Token::Bang) => {
                    self.bump();
                    node = Expr::Fact(Box::new(node));
                }
                Some(Token::Percent) => {
                    self.bump();
                    node = Expr::Percent(Box::new(node));
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// `atom` — the highest-precedence forms.
    fn parse_atom(&mut self) -> Result<Expr, EvalError> {
        let _guard = DepthGuard::new(self.depth)?;
        match self.peek() {
            Some(Token::Number(n)) => {
                let n = *n;
                self.bump();
                Ok(Expr::Num(n))
            }
            Some(Token::Pi) => {
                self.bump();
                Ok(Expr::Pi)
            }
            Some(Token::E) => {
                self.bump();
                Ok(Expr::E)
            }
            Some(Token::Sqrt) => {
                self.bump();
                // √ is a prefix over a single factor (a `unary`), so `√9+7`
                // is `(√9)+7` and `√9` alone is `3`. Manual parens still work
                // because `(` is an atom: `√(9+7)`.
                let operand = self.parse_unary()?;
                Ok(Expr::Sqrt(Box::new(operand)))
            }
            Some(Token::LParen) => {
                self.bump();
                let inner = self.parse_add()?;
                self.expect_rparen()?;
                Ok(Expr::Paren(Box::new(inner)))
            }
            Some(Token::Func(f)) => {
                let f = *f;
                self.bump();
                // The `(` is required in canonical form (the UI always inserts
                // it with the function). Tolerate its absence defensively.
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.bump();
                }
                let arg = self.parse_add()?;
                self.expect_rparen()?;
                Ok(Expr::Call(f, Box::new(arg)))
            }
            Some(Token::Func2(f)) => {
                let f = *f;
                self.bump();
                // Like the one-arg form, tolerate a missing `(` defensively.
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.bump();
                }
                let a = self.parse_add()?;
                // The comma between the two args is REQUIRED; its absence is a
                // syntax error (unlike the optional paren / auto-close).
                if matches!(self.peek(), Some(Token::Comma)) {
                    self.bump();
                } else {
                    return Err(EvalError::Syntax);
                }
                let b = self.parse_add()?;
                self.expect_rparen()?;
                Ok(Expr::Call2(f, Box::new(a), Box::new(b)))
            }
            // A trailing binary operator, a stray `)` etc. all land here.
            _ => Err(EvalError::Syntax),
        }
    }

    /// Consume a `)` if present; treat EOF as an implicit close (auto-close).
    /// Any *other* token where `)` is expected is a syntax error.
    fn expect_rparen(&mut self) -> Result<(), EvalError> {
        match self.peek() {
            Some(Token::RParen) => {
                self.bump();
                Ok(())
            }
            None => Ok(()), // auto-close at end of input
            _ => Err(EvalError::Syntax),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::lexer::tokenize;

    fn ast(s: &str) -> Expr {
        parse(&tokenize(s).unwrap()).unwrap()
    }

    #[test]
    fn additive_left_assoc() {
        // 1-2-3 = (1-2)-3
        let e = ast("1-2-3");
        assert_eq!(
            e,
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Bin(
                    BinOp::Sub,
                    Box::new(Expr::Num(1.0)),
                    Box::new(Expr::Num(2.0))
                )),
                Box::new(Expr::Num(3.0))
            )
        );
    }

    #[test]
    fn power_right_assoc() {
        // 2^3^2 = 2^(3^2)
        let e = ast("2^3^2");
        assert_eq!(
            e,
            Expr::Bin(
                BinOp::Pow,
                Box::new(Expr::Num(2.0)),
                Box::new(Expr::Bin(
                    BinOp::Pow,
                    Box::new(Expr::Num(3.0)),
                    Box::new(Expr::Num(2.0))
                ))
            )
        );
    }

    #[test]
    fn unary_minus_base_of_power() {
        // -2^2 must be (-2)^2, not -(2^2).
        let e = ast("-2^2");
        assert_eq!(
            e,
            Expr::Bin(
                BinOp::Pow,
                Box::new(Expr::Neg(Box::new(Expr::Num(2.0)))),
                Box::new(Expr::Num(2.0))
            )
        );
    }

    #[test]
    fn trailing_operator_is_syntax_error() {
        assert_eq!(parse(&tokenize("5+").unwrap()), Err(EvalError::Syntax));
    }

    #[test]
    fn auto_close_paren() {
        // 2*(3+4  parses to  2*(3+4)
        let e = ast("2*(3+4");
        assert_eq!(
            e,
            Expr::Bin(
                BinOp::Mul,
                Box::new(Expr::Num(2.0)),
                Box::new(Expr::Paren(Box::new(Expr::Bin(
                    BinOp::Add,
                    Box::new(Expr::Num(3.0)),
                    Box::new(Expr::Num(4.0))
                ))))
            )
        );
    }

    #[test]
    fn func_auto_close() {
        // cos(π  parses to cos(π)
        let e = ast("cos(\u{03C0}");
        assert_eq!(e, Expr::Call(FuncName::Cos, Box::new(Expr::Pi)));
    }

    // ---- Two-arg calls ----

    #[test]
    fn two_arg_call_shape() {
        let e = ast("nPr(5,2)");
        assert_eq!(
            e,
            Expr::Call2(
                Func2Name::Npr,
                Box::new(Expr::Num(5.0)),
                Box::new(Expr::Num(2.0))
            )
        );
    }

    #[test]
    fn two_arg_missing_comma_is_syntax_error() {
        assert_eq!(parse(&tokenize("nPr(5)").unwrap()), Err(EvalError::Syntax));
    }

    #[test]
    fn sqrt_prefix_binds_one_factor() {
        // √9+7 = (√9)+7
        let e = ast("\u{221A}9+7");
        assert_eq!(
            e,
            Expr::Bin(
                BinOp::Add,
                Box::new(Expr::Sqrt(Box::new(Expr::Num(9.0)))),
                Box::new(Expr::Num(7.0))
            )
        );
    }

    #[test]
    fn percent_is_postfix_marker() {
        let e = ast("50%");
        assert_eq!(e, Expr::Percent(Box::new(Expr::Num(50.0))));
    }

    #[test]
    fn stray_rparen_errors() {
        assert!(parse(&tokenize("5)").unwrap()).is_err());
    }

    #[test]
    fn adjacent_numbers_are_syntax_error() {
        // `3.5.2` → two number tokens, no operator → parser rejects it.
        assert!(parse(&tokenize("3.5.2").unwrap()).is_err());
    }

    #[test]
    fn deeply_nested_parens_error_no_overflow() {
        // 5000 open parens would recurse ~5000 deep; the cap must turn this
        // into a syntax error instead of a stack overflow / SIGABRT.
        let s = "(".repeat(5000) + "1";
        assert_eq!(parse(&tokenize(&s).unwrap()), Err(EvalError::Syntax));
    }

    #[test]
    fn deeply_nested_neg_error_no_overflow() {
        // A long run of unary minuses recurses through parse_neg_base; the cap
        // must reject it rather than overflow.
        let s = "-".repeat(5000) + "1";
        assert!(parse(&tokenize(&s).unwrap()).is_err());
    }

    #[test]
    fn deeply_nested_power_error_no_overflow() {
        // A right-assoc power chain `2^2^2^…^2` re-enters parse_neg_base /
        // parse_atom for each new base; the cap must reject it, not overflow.
        let s = "2^".repeat(5000) + "2";
        assert!(parse(&tokenize(&s).unwrap()).is_err());
    }

    #[test]
    fn reasonable_nesting_still_evaluates() {
        use crate::engine::{evaluate, AngleUnit};
        // The spec requires 50 nested parens to still evaluate. With the guard
        // applied only at parse_atom + parse_neg_base, each paren level costs
        // ~1-2 depth-units, so 50 levels stays well under the 256 cap.
        let s = "(".repeat(50) + "1" + &")".repeat(50);
        assert_eq!(parse(&tokenize(&s).unwrap()).is_ok(), true);
        assert_eq!(evaluate(&s, AngleUnit::Rad), Ok(1.0));
    }
}
