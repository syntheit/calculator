//! AST evaluation: angle unit, functions, factorial, and Google percent
//! semantics.
//!
//! The evaluator walks the [`Expr`] tree produced by [`crate::engine::parser`].
//! Two behaviours are subtle and pinned by golden tests:
//!
//! * **Angle unit.** In [`AngleUnit::Deg`], `sin`/`cos`/`tan` convert their
//!   argument from degrees to radians before the call, and `asin`/`acos`/`atan`
//!   convert their radian result back to degrees.
//!
//! * **Google percent.** `%` is kept as an [`Expr::Percent`] marker in the AST
//!   and resolved contextually here. As the direct right operand of an additive
//!   `+`/`-` (and not defeated by parentheses) it means "percent *of the left
//!   side*": `100+10% = 110`, `100−10% = 90`. In every other position it means
//!   plain `× 1/100`: `50% = 0.5`, `100×10% = 10`, `100÷50% = 200`,
//!   `10%+5 = 5.1`, `100+(10)% = 100.1`.
//!
//! * **Angle-independent functions.** The hyperbolic functions plus `abs`,
//!   `log2`, and `cbrt` ignore the angle unit and operate on the raw argument.

use crate::engine::parser::{BinOp, Expr};
use crate::engine::EvalError;
use crate::engine::lexer::{Func2Name, FuncName};

/// Whether trigonometric arguments/results are in radians or degrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AngleUnit {
    Rad,
    Deg,
}

/// Evaluate `expr` under the given angle unit, returning the numeric result or
/// a typed [`EvalError`].
pub fn eval(expr: &Expr, angle: AngleUnit) -> Result<f64, EvalError> {
    let v = eval_node(expr, angle)?;
    finalize(v)
}

/// Map a raw `f64` to an error if it is non-finite, else return it.
fn finalize(v: f64) -> Result<f64, EvalError> {
    if v.is_nan() {
        Err(EvalError::NotANumber)
    } else if v.is_infinite() {
        Err(EvalError::Overflow)
    } else {
        Ok(v)
    }
}

fn eval_node(expr: &Expr, angle: AngleUnit) -> Result<f64, EvalError> {
    match expr {
        Expr::Num(n) => Ok(*n),
        Expr::Pi => Ok(std::f64::consts::PI),
        Expr::E => Ok(std::f64::consts::E),
        Expr::Paren(inner) => eval_node(inner, angle),
        Expr::Neg(inner) => Ok(-eval_node(inner, angle)?),
        Expr::Sqrt(inner) => {
            let x = eval_node(inner, angle)?;
            if x < 0.0 {
                // √ of a negative is not a real number.
                return Err(EvalError::Domain);
            }
            Ok(x.sqrt())
        }
        Expr::Fact(inner) => factorial(eval_node(inner, angle)?),
        // A bare percent (not caught by the additive special-case) is × 1/100.
        Expr::Percent(inner) => Ok(eval_node(inner, angle)? / 100.0),
        Expr::Call(f, arg) => eval_call(*f, eval_node(arg, angle)?, angle),
        Expr::Call2(f, a, b) => eval_call2(*f, eval_node(a, angle)?, eval_node(b, angle)?, angle),
        Expr::Bin(op, lhs, rhs) => eval_bin(*op, lhs, rhs, angle),
    }
}

/// Evaluate a binary node, applying the Google additive-percent rule when the
/// right operand is a terminal (non-parenthesised) percent.
fn eval_bin(op: BinOp, lhs: &Expr, rhs: &Expr, angle: AngleUnit) -> Result<f64, EvalError> {
    // Additive percent: `a + b%` / `a − b%` → `a * (1 ± b/100)`.
    // Defeated by parentheses: `(10)%` carries a `Paren` inside the `Percent`.
    if matches!(op, BinOp::Add | BinOp::Sub) {
        if let Expr::Percent(inner) = rhs {
            if !is_parenthesised(inner) {
                let a = eval_node(lhs, angle)?;
                let b = eval_node(inner, angle)?;
                let factor = if op == BinOp::Add {
                    1.0 + b / 100.0
                } else {
                    1.0 - b / 100.0
                };
                return Ok(a * factor);
            }
        }
    }

    let a = eval_node(lhs, angle)?;
    let b = eval_node(rhs, angle)?;
    match op {
        BinOp::Add => Ok(a + b),
        BinOp::Sub => Ok(a - b),
        BinOp::Mul => Ok(a * b),
        BinOp::Div => {
            if b == 0.0 {
                Err(EvalError::DivideByZero)
            } else {
                Ok(a / b)
            }
        }
        BinOp::Pow => {
            let r = a.powf(b);
            // e.g. (-1)^0.5 → NaN; surface as a domain error rather than
            // leaking a NaN through to the caller.
            if r.is_nan() {
                Err(EvalError::Domain)
            } else {
                Ok(r)
            }
        }
    }
}

/// True if `inner` is wrapped in an explicit paren node (possibly after peeling
/// a single layer). Used only to defeat the additive-percent rule.
fn is_parenthesised(inner: &Expr) -> bool {
    matches!(inner, Expr::Paren(_))
}

/// Evaluate a named function, honouring the angle unit for trig/inverse-trig.
fn eval_call(f: FuncName, x: f64, angle: AngleUnit) -> Result<f64, EvalError> {
    let deg = angle == AngleUnit::Deg;
    let to_rad = |v: f64| v * std::f64::consts::PI / 180.0;
    let to_deg = |v: f64| v * 180.0 / std::f64::consts::PI;
    match f {
        FuncName::Sin => Ok(if deg { to_rad(x).sin() } else { x.sin() }),
        FuncName::Cos => Ok(if deg { to_rad(x).cos() } else { x.cos() }),
        FuncName::Tan => Ok(if deg { to_rad(x).tan() } else { x.tan() }),
        FuncName::Asin => {
            if !(-1.0..=1.0).contains(&x) {
                return Err(EvalError::Domain);
            }
            let r = x.asin();
            Ok(if deg { to_deg(r) } else { r })
        }
        FuncName::Acos => {
            if !(-1.0..=1.0).contains(&x) {
                return Err(EvalError::Domain);
            }
            let r = x.acos();
            Ok(if deg { to_deg(r) } else { r })
        }
        FuncName::Atan => {
            let r = x.atan();
            Ok(if deg { to_deg(r) } else { r })
        }
        FuncName::Ln => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            Ok(x.ln())
        }
        FuncName::Log => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            Ok(x.log10())
        }
        FuncName::Exp => Ok(x.exp()),
        FuncName::Sinh => Ok(x.sinh()),
        FuncName::Cosh => Ok(x.cosh()),
        FuncName::Tanh => Ok(x.tanh()),
        FuncName::Asinh => Ok(x.asinh()),
        FuncName::Acosh => {
            if x < 1.0 {
                return Err(EvalError::Domain);
            }
            Ok(x.acosh())
        }
        FuncName::Atanh => {
            if !(-1.0..1.0).contains(&x) {
                return Err(EvalError::Domain);
            }
            Ok(x.atanh())
        }
        FuncName::Abs => Ok(x.abs()),
        FuncName::Log2 => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            Ok(x.log2())
        }
        FuncName::Cbrt => Ok(x.cbrt()),
    }
}

/// Evaluate a two-argument named function. These are all angle-independent, so
/// `_angle` is unused.
fn eval_call2(f: Func2Name, a: f64, b: f64, _angle: AngleUnit) -> Result<f64, EvalError> {
    match f {
        Func2Name::Npr => {
            let n = a;
            let r = b;
            if n < 0.0 || n.fract() != 0.0 || r < 0.0 || r.fract() != 0.0 || r > n {
                return Err(EvalError::Domain);
            }
            // nPr(n, r) = product over i in 0..r of (n - i). Empty product = 1.
            let r_count = r as u64;
            let mut acc = 1.0f64;
            for i in 0..r_count {
                acc *= n - i as f64;
                // Bail the instant the product overflows to +/-inf (or NaN).
                // acc only grows in magnitude here, so once it is non-finite
                // it stays so; the early return bounds the loop to a few dozen
                // iterations for huge inputs instead of up to `r` iterations.
                if !acc.is_finite() {
                    return Err(EvalError::Overflow);
                }
            }
            let result = acc.round();
            if result.is_infinite() || result.is_nan() {
                return Err(EvalError::Overflow);
            }
            Ok(result)
        }
        Func2Name::Ncr => {
            let n = a;
            let r = b;
            if n < 0.0 || n.fract() != 0.0 || r < 0.0 || r.fract() != 0.0 || r > n {
                return Err(EvalError::Domain);
            }
            // nCr(n, r): use the smaller of r and n-r, and divide inside the loop
            // to keep magnitudes bounded and near integer-exact.
            let r_count = r as u64;
            let n_count = n as u64;
            let k = r_count.min(n_count - r_count);
            let mut acc = 1.0f64;
            for i in 1..=k {
                acc = acc * (n - k as f64 + i as f64) / i as f64;
                // nCr grows extremely fast; for huge inputs acc overflows to
                // +/-inf within a few dozen iterations. Bail immediately so the
                // loop is bounded rather than running up to k = min(r, n-r)
                // iterations (which is ~1e8 for nCr(1e9, 1e8)).
                if !acc.is_finite() {
                    return Err(EvalError::Overflow);
                }
            }
            let result = acc.round();
            if result.is_infinite() || result.is_nan() {
                return Err(EvalError::Overflow);
            }
            Ok(result)
        }
        Func2Name::Root => {
            // `a` is the degree, `b` the radicand.
            if a == 0.0 {
                return Err(EvalError::Domain);
            }
            if b >= 0.0 {
                let r = b.powf(1.0 / a);
                if r.is_nan() {
                    Err(EvalError::Domain)
                } else {
                    Ok(r)
                }
            } else if a.fract() == 0.0 && (a as i64) % 2 != 0 {
                // Odd integer degree of a negative radicand has a real root.
                let r = -(b.abs().powf(1.0 / a));
                if r.is_nan() {
                    Err(EvalError::Domain)
                } else {
                    Ok(r)
                }
            } else {
                // Even integer degree, or non-integer degree, of a negative
                // radicand has no real result.
                Err(EvalError::Domain)
            }
        }
        Func2Name::Logb => {
            if a <= 0.0 || a == 1.0 || b <= 0.0 {
                return Err(EvalError::Domain);
            }
            Ok(b.log(a))
        }
    }
}

/// Factorial. Non-negative integers are computed exactly by a product
/// (`0! = 1`). Any other value is rejected as a domain error (we deliberately do
/// not pull in a gamma implementation; the button is only ever pressed on whole
/// numbers in practice).
fn factorial(x: f64) -> Result<f64, EvalError> {
    if x < 0.0 || x.fract() != 0.0 {
        return Err(EvalError::Domain);
    }
    // 171! overflows f64; guard so we return Overflow rather than +inf silently.
    if x > 170.0 {
        return Err(EvalError::Overflow);
    }
    let n = x as u64;
    let mut acc = 1.0f64;
    for k in 2..=n {
        acc *= k as f64;
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::lexer::tokenize;
    use crate::engine::parser::parse;

    fn ev(s: &str, angle: AngleUnit) -> Result<f64, EvalError> {
        let toks = tokenize(s)?;
        let ast = parse(&toks)?;
        eval(&ast, angle)
    }

    fn ok(s: &str) -> f64 {
        ev(s, AngleUnit::Rad).unwrap()
    }

    fn ok_deg(s: &str) -> f64 {
        ev(s, AngleUnit::Deg).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ---- Percent (the signature behaviour) --------------------------------

    #[test]
    fn percent_additive_add() {
        assert!(close(ok("100+10%"), 110.0));
    }

    #[test]
    fn percent_additive_sub() {
        assert!(close(ok("100-10%"), 90.0));
    }

    #[test]
    fn percent_bare() {
        assert!(close(ok("50%"), 0.5));
    }

    #[test]
    fn percent_mul() {
        assert!(close(ok("100*10%"), 10.0));
        assert!(close(ok("100*50%"), 50.0));
    }

    #[test]
    fn percent_div() {
        assert!(close(ok("100/50%"), 200.0));
    }

    #[test]
    fn percent_parens_defeat_additive() {
        assert!(close(ok("100+(10)%"), 100.1));
    }

    #[test]
    fn percent_leading() {
        assert!(close(ok("10%+5"), 5.1));
    }

    // ---- Precedence / associativity ---------------------------------------

    #[test]
    fn power_right_assoc() {
        assert!(close(ok("2^3^2"), 512.0));
    }

    #[test]
    fn unary_minus_tighter_than_power() {
        assert!(close(ok("-2^2"), 4.0));
    }

    #[test]
    fn mul_before_add() {
        assert!(close(ok("2+3*4"), 14.0));
    }

    #[test]
    fn parens_group() {
        assert!(close(ok("(2+3)*4"), 20.0));
    }

    // ---- Implicit multiplication ------------------------------------------

    #[test]
    fn implicit_two_pi() {
        assert!(close(ok("2\u{03C0}"), std::f64::consts::TAU));
    }

    #[test]
    fn implicit_two_paren() {
        assert!(close(ok("2(3)"), 6.0));
    }

    #[test]
    fn implicit_paren_paren() {
        assert!(close(ok("(1+2)(3)"), 9.0));
    }

    #[test]
    fn implicit_two_sqrt() {
        assert!(close(ok("2\u{221A}9"), 6.0));
    }

    // ---- Sqrt prefix -------------------------------------------------------

    #[test]
    fn sqrt_plain() {
        assert!(close(ok("\u{221A}9"), 3.0));
    }

    #[test]
    fn sqrt_binds_one_factor() {
        assert!(close(ok("\u{221A}9+7"), 10.0));
    }

    #[test]
    fn sqrt_with_parens() {
        assert!(close(ok("\u{221A}(9+7)"), 4.0));
    }

    // ---- Functions (Rad) ---------------------------------------------------

    #[test]
    fn trig_rad() {
        assert!(close(ok("sin(0)"), 0.0));
        assert!(close(ok("cos(\u{03C0})"), -1.0));
        assert!(close(ok("tan(0)"), 0.0));
        assert!(close(ok("ln(e)"), 1.0));
        assert!(close(ok("log(10)"), 1.0));
        assert!(close(ok("exp(0)"), 1.0));
    }

    #[test]
    fn sin_half_pi_rad() {
        assert!(close(ok("sin(\u{03C0}/2)"), 1.0));
    }

    // ---- Functions (Deg) ---------------------------------------------------

    #[test]
    fn trig_deg() {
        assert!(close(ok_deg("sin(90)"), 1.0));
        assert!(close(ok_deg("cos(0)"), 1.0));
        assert!(close(ok_deg("asin(1)"), 90.0));
        assert!(close(ok_deg("atan(1)"), 45.0));
    }

    // ---- Factorial ---------------------------------------------------------

    #[test]
    fn factorials() {
        assert!(close(ok("5!"), 120.0));
        assert!(close(ok("0!"), 1.0));
        assert!(close(ok("3!+1"), 7.0));
    }

    #[test]
    fn factorial_of_negative_is_domain() {
        assert_eq!(ev("(-1)!", AngleUnit::Rad), Err(EvalError::Domain));
    }

    // ---- Auto-close --------------------------------------------------------

    #[test]
    fn auto_close_mul() {
        assert!(close(ok("2*(3+4"), 14.0));
    }

    #[test]
    fn auto_close_func() {
        assert!(close(ok("cos(\u{03C0}"), -1.0));
    }

    // ---- Errors ------------------------------------------------------------

    #[test]
    fn divide_by_zero() {
        assert_eq!(ev("5/0", AngleUnit::Rad), Err(EvalError::DivideByZero));
    }

    #[test]
    fn trailing_operator_syntax() {
        assert_eq!(ev("5+", AngleUnit::Rad), Err(EvalError::Syntax));
    }

    #[test]
    fn ln_of_zero_is_domain() {
        assert_eq!(ev("ln(0)", AngleUnit::Rad), Err(EvalError::Domain));
    }

    // ---- hyperbolic --------------------------------------------------------
    #[test]
    fn hyperbolic_basic() {
        assert!(close(ok("sinh(0)"), 0.0));
        assert!(close(ok("cosh(0)"), 1.0));
        assert!(close(ok("tanh(0)"), 0.0));
        assert!(close(ok("sinh(1)"), 1.1752011936438014));
        assert!(close(ok("asinh(0)"), 0.0));
        assert!(close(ok("acosh(1)"), 0.0));
        assert!(close(ok("atanh(0)"), 0.0));
    }

    #[test]
    fn hyperbolic_angle_independent() {
        // sinh must be identical in Deg and Rad.
        assert_eq!(
            ev("sinh(1)", AngleUnit::Deg).unwrap(),
            ev("sinh(1)", AngleUnit::Rad).unwrap()
        );
    }

    #[test]
    fn acosh_below_one_is_domain() {
        assert_eq!(ev("acosh(0)", AngleUnit::Rad), Err(EvalError::Domain));
    }

    // ---- abs / log2 / cbrt -------------------------------------------------
    #[test]
    fn abs_log2_cbrt() {
        assert!(close(ok("abs(-5)"), 5.0));
        assert!(close(ok("log2(8)"), 3.0));
        assert!(close(ok("log2(1)"), 0.0));
        assert!(close(ok("cbrt(27)"), 3.0));
        assert!(close(ok("cbrt(-8)"), -2.0));
    }

    #[test]
    fn log2_of_zero_is_domain() {
        assert_eq!(ev("log2(0)", AngleUnit::Rad), Err(EvalError::Domain));
    }

    // ---- two-arg functions -------------------------------------------------
    #[test]
    fn combinatorics() {
        assert!(close(ok("nCr(5,2)"), 10.0));
        assert!(close(ok("nCr(10,3)"), 120.0));
        assert!(close(ok("nCr(5,5)"), 1.0));
        assert!(close(ok("nPr(5,2)"), 20.0));
        assert!(close(ok("nPr(5,0)"), 1.0));
    }

    #[test]
    fn combinatorics_domain() {
        assert_eq!(ev("nCr(2,5)", AngleUnit::Rad), Err(EvalError::Domain));
    }

    #[test]
    fn nth_root_and_logb() {
        assert!(close(ok("root(3,27)"), 3.0));
        assert!(close(ok("root(2,9)"), 3.0));
        assert!(close(ok("logb(2,8)"), 3.0));
        assert!(close(ok("logb(10,1000)"), 3.0));
    }

    #[test]
    fn combinatorics_large_no_false_overflow() {
        // These have tiny results; the old full-factorial impl wrongly hit the
        // 170! overflow guard. Bounded products must give exact integers.
        assert!(close(ok("nCr(171,1)"), 171.0));
        assert!(close(ok("nPr(171,1)"), 171.0));
        assert!(close(ok("nCr(200,2)"), 19900.0));
        assert!(close(ok("nPr(200,2)"), 39800.0));
    }

    #[test]
    fn combinatorics_exact() {
        assert!(close(ok("nCr(52,5)"), 2598960.0));
        assert!(close(ok("nCr(10,3)"), 120.0));
        assert_eq!(ev("nCr(2,5)", AngleUnit::Rad), Err(EvalError::Domain));
    }

    #[test]
    fn huge_npr_ncr_overflow_fast() {
        // These must NOT hang: the in-loop finite guard bails within a few
        // dozen iterations once the accumulator overflows to infinity, instead
        // of iterating up to r (~1e8). nPr(1e9, 1e8) and nCr(1e9, 1e8) both
        // overflow f64 (max ~1.8e308), so both return Overflow essentially
        // instantly. If the guard is wrong this test hangs, which is the signal.
        assert_eq!(
            ev("nPr(1000000000,100000000)", AngleUnit::Rad),
            Err(EvalError::Overflow)
        );
        assert_eq!(
            ev("nCr(1000000000,100000000)", AngleUnit::Rad),
            Err(EvalError::Overflow)
        );
    }

    #[test]
    fn odd_root_of_negative() {
        assert!(close(ok("root(3,-8)"), -2.0));
        assert!(close(ok("root(5,-32)"), -2.0));
        assert_eq!(ev("root(2,-4)", AngleUnit::Rad), Err(EvalError::Domain));
        assert!(close(ok("root(3,27)"), 3.0));
        assert!(close(ok("root(2,4)"), 2.0));
    }
}
