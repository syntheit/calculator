//! Financial engine.
//!
//! Pure-Rust Time-Value-of-Money (TVM) plus finance routines for the
//! calculator's financial mode. No GTK, no I/O; only `std` and `thiserror`.
//!
//! All formulas match GNOME Calculator's financial mode (see
//! `financial.vala`) plus standard TVM identities. Every public function
//! returns `Result<f64, FinError>`, guards non-finite inputs up front, and
//! routes its final value through [`finite`] so callers never receive a
//! `NaN`/`inf` from a valid-looking call.

// This engine deliberately exposes the full TVM/finance surface (solve-for-any-
// variable, annuities, all depreciation methods) even though the current UI wires
// only a subset. The rest is covered by unit tests and kept as stable public API.
#![allow(dead_code)]

/// Errors surfaced by the financial engine.
///
/// Plain unit variants only (no payload) so the enum can derive `Eq`/`Copy`;
/// messages are short and fit a calculator display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FinError {
    /// A rate argument was zero where the formula requires a non-zero rate.
    #[error("Rate can't be zero")]
    ZeroRate,
    /// Input was `NaN`, infinite, or otherwise unusable.
    #[error("Invalid input")]
    Invalid,
    /// A value that must be strictly positive was not.
    #[error("Value must be positive")]
    NonPositive,
    /// Division by zero (explicit or via a zero denominator).
    #[error("Can't divide by 0")]
    DivByZero,
    /// Argument fell outside a function's mathematical domain (e.g. `ln`).
    #[error("Out of domain")]
    Domain,
}

/// Returns `Ok(x)` when `x` is finite, else [`FinError::Invalid`].
fn finite(x: f64) -> Result<f64, FinError> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(FinError::Invalid)
    }
}

/// Returns `Ok(())` when every arg is finite, else [`FinError::Invalid`].
fn check_finite(args: &[f64]) -> Result<(), FinError> {
    for &a in args {
        if !a.is_finite() {
            return Err(FinError::Invalid);
        }
    }
    Ok(())
}

/// Compound future value: `pv * (1 + rate)^n`.
pub fn compound_fv(pv: f64, rate: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[pv, rate, n])?;
    finite(pv * (1.0 + rate).powf(n))
}

/// Compound present value: `fv / (1 + rate)^n`.
///
/// Errors [`FinError::DivByZero`] when `(1 + rate)^n == 0`.
pub fn compound_pv(fv: f64, rate: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[fv, rate, n])?;
    let factor = (1.0 + rate).powf(n);
    if factor == 0.0 {
        return Err(FinError::DivByZero);
    }
    finite(fv / factor)
}

/// Number of periods (GNOME `Ctrm`): `ln(fv/pv) / ln(1 + rate)`.
///
/// Errors: [`FinError::DivByZero`] if `pv == 0`; [`FinError::Domain`] if
/// `fv/pv <= 0` or `1 + rate <= 0` (both `ln`-domain violations);
/// [`FinError::ZeroRate`] if `rate == 0` (denominator `ln(1) == 0`).
pub fn compound_n(pv: f64, fv: f64, rate: f64) -> Result<f64, FinError> {
    check_finite(&[pv, fv, rate])?;
    if pv == 0.0 {
        return Err(FinError::DivByZero);
    }
    let ratio = fv / pv;
    if ratio <= 0.0 {
        return Err(FinError::Domain);
    }
    let base = 1.0 + rate;
    if base <= 0.0 {
        return Err(FinError::Domain);
    }
    if base == 1.0 {
        return Err(FinError::ZeroRate);
    }
    finite(ratio.ln() / base.ln())
}

/// Periodic rate (GNOME `Rate`): `(fv/pv)^(1/n) - 1`.
///
/// Errors: [`FinError::DivByZero`] if `pv == 0` or `n == 0` (the latter would
/// make `1/n` infinite).
pub fn compound_rate(pv: f64, fv: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[pv, fv, n])?;
    if pv == 0.0 {
        return Err(FinError::DivByZero);
    }
    if n == 0.0 {
        return Err(FinError::DivByZero);
    }
    finite((fv / pv).powf(1.0 / n) - 1.0)
}

/// Loan payment (GNOME `Pmt`): `principal * rate / (1 - (rate + 1)^-n)`.
///
/// Special case: when `rate == 0` the payment is `principal / n` (errors
/// [`FinError::DivByZero`] if `n == 0`). Otherwise a zero denominator errors
/// [`FinError::DivByZero`].
pub fn loan_payment(principal: f64, rate: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[principal, rate, n])?;
    if rate == 0.0 {
        if n == 0.0 {
            return Err(FinError::DivByZero);
        }
        return finite(principal / n);
    }
    let denom = 1.0 - (rate + 1.0).powf(-n);
    if denom == 0.0 {
        return Err(FinError::DivByZero);
    }
    finite(principal * (rate / denom))
}

/// Annuity future value (GNOME `Fv`): `pmt * ((1 + rate)^n - 1) / rate`.
///
/// Special case: when `rate == 0` the future value is `pmt * n`.
pub fn annuity_fv(pmt: f64, rate: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[pmt, rate, n])?;
    if rate == 0.0 {
        return finite(pmt * n);
    }
    finite(pmt * ((1.0 + rate).powf(n) - 1.0) / rate)
}

/// Annuity present value (GNOME `Pv`): `pmt * (1 - (1 + rate)^-n) / rate`.
///
/// Special case: when `rate == 0` the present value is `pmt * n`.
pub fn annuity_pv(pmt: f64, rate: f64, n: f64) -> Result<f64, FinError> {
    check_finite(&[pmt, rate, n])?;
    if rate == 0.0 {
        return finite(pmt * n);
    }
    finite(pmt * (1.0 - (1.0 + rate).powf(-n)) / rate)
}

/// Annuity term (GNOME `Term`): `ln(1 + fv*rate/pmt) / ln(1 + rate)`.
///
/// Errors: [`FinError::DivByZero`] if `pmt == 0`; [`FinError::ZeroRate`] if
/// `rate == 0`; [`FinError::Domain`] if `1 + rate <= 0` or
/// `1 + fv*rate/pmt <= 0` (both `ln`-domain violations).
pub fn annuity_n(pmt: f64, fv: f64, rate: f64) -> Result<f64, FinError> {
    check_finite(&[pmt, fv, rate])?;
    if pmt == 0.0 {
        return Err(FinError::DivByZero);
    }
    if rate == 0.0 {
        return Err(FinError::ZeroRate);
    }
    let base = 1.0 + rate;
    if base <= 0.0 {
        return Err(FinError::Domain);
    }
    let arg = 1.0 + fv * rate / pmt;
    if arg <= 0.0 {
        return Err(FinError::Domain);
    }
    finite(arg.ln() / base.ln())
}

/// Simple interest earned: `principal * rate * t`.
pub fn simple_interest(principal: f64, rate: f64, t: f64) -> Result<f64, FinError> {
    check_finite(&[principal, rate, t])?;
    finite(principal * rate * t)
}

/// Simple-interest total (principal + interest): `principal * (1 + rate*t)`.
pub fn simple_interest_total(principal: f64, rate: f64, t: f64) -> Result<f64, FinError> {
    check_finite(&[principal, rate, t])?;
    finite(principal * (1.0 + rate * t))
}

/// Price from gross margin (GNOME `Gpm`): `cost / (1 - margin)`.
///
/// Errors [`FinError::DivByZero`] when `margin >= 1` or `1 - margin == 0`.
pub fn gross_margin_price(cost: f64, margin: f64) -> Result<f64, FinError> {
    check_finite(&[cost, margin])?;
    if margin >= 1.0 {
        return Err(FinError::DivByZero);
    }
    let denom = 1.0 - margin;
    if denom == 0.0 {
        return Err(FinError::DivByZero);
    }
    finite(cost / denom)
}

/// Price from markup: `cost * (1 + markup)`.
pub fn markup_price(cost: f64, markup: f64) -> Result<f64, FinError> {
    check_finite(&[cost, markup])?;
    finite(cost * (1.0 + markup))
}

/// Straight-line depreciation (GNOME `Sln`): `(cost - salvage) / life`.
///
/// Errors [`FinError::DivByZero`] when `life == 0`.
pub fn depreciation_sln(cost: f64, salvage: f64, life: f64) -> Result<f64, FinError> {
    check_finite(&[cost, salvage, life])?;
    if life == 0.0 {
        return Err(FinError::DivByZero);
    }
    finite((cost - salvage) / life)
}

/// Sum-of-years'-digits depreciation (GNOME `Syd`):
/// `(cost - salvage) * (life - period + 1) / (life * (life + 1) / 2)`.
///
/// `life` must be positive, else [`FinError::DivByZero`]. `period` must be an
/// integer in `1..=life`, else [`FinError::Invalid`].
pub fn depreciation_syd(
    cost: f64,
    salvage: f64,
    life: f64,
    period: f64,
) -> Result<f64, FinError> {
    check_finite(&[cost, salvage, life, period])?;
    if life <= 0.0 {
        return Err(FinError::DivByZero);
    }
    // period must be a positive integer within 1..=life
    if period.fract() != 0.0 || period < 1.0 || period > life {
        return Err(FinError::Invalid);
    }
    finite((cost - salvage) * (life - period + 1.0) / (life * (life + 1.0) / 2.0))
}

/// Double-declining-balance depreciation (GNOME `Ddb`).
///
/// Iterates `period` times, each step depreciating `2/life` of the remaining
/// book value, clamped so the book value never drops below `salvage`, and
/// returns the last step's amount.
///
/// `life` must be positive, else [`FinError::DivByZero`]. `period` must be an
/// integer in `1..=life`, else [`FinError::Invalid`].
pub fn depreciation_ddb(cost: f64, salvage: f64, life: f64, period: f64) -> Result<f64, FinError> {
    check_finite(&[cost, salvage, life, period])?;
    if life <= 0.0 {
        return Err(FinError::DivByZero);
    }
    // period must be a positive integer within 1..=life
    if period.fract() != 0.0 || period < 1.0 || period > life {
        return Err(FinError::Invalid);
    }
    let rate = 2.0 / life;
    let count = period as i64;
    let mut bv = cost;
    let mut z = 0.0;
    for _ in 0..count {
        // Depreciate `rate` of the remaining book value, but never take the
        // book value below salvage.
        z = (rate * bv).min((bv - salvage).max(0.0));
        bv -= z;
    }
    finite(z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn loan_payment_golden() {
        let p = loan_payment(10_000.0, 0.05 / 12.0, 360.0).unwrap();
        assert!(close(p, 53.6822, 0.01), "got {p}");
    }

    #[test]
    fn loan_payment_zero_rate() {
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(loan_payment(1200.0, 0.0, 12.0).unwrap(), 100.0);
        }
    }

    #[test]
    fn compound_roundtrip() {
        let fv = compound_fv(1000.0, 0.05, 10.0).unwrap();
        assert!(close(fv, 1628.894627, 1e-4), "fv {fv}");
        let pv = compound_pv(1628.894627, 0.05, 10.0).unwrap();
        assert!(close(pv, 1000.0, 1e-4), "pv {pv}");
    }

    #[test]
    fn compound_n_golden() {
        let n = compound_n(1000.0, 2000.0, 0.05).unwrap();
        assert!(close(n, 14.2067, 1e-3), "got {n}");
    }

    #[test]
    fn compound_rate_golden() {
        let r = compound_rate(1000.0, 2000.0, 10.0).unwrap();
        assert!(close(r, 0.07177, 1e-4), "got {r}");
    }

    #[test]
    fn annuity_golden() {
        let fv = annuity_fv(100.0, 0.05, 10.0).unwrap();
        assert!(close(fv, 1257.789, 1e-2), "fv {fv}");
        let pv = annuity_pv(100.0, 0.05, 10.0).unwrap();
        assert!(close(pv, 772.173, 1e-2), "pv {pv}");
        let n = annuity_n(100.0, 1257.789, 0.05).unwrap();
        assert!(close(n, 10.0, 1e-2), "n {n}");
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn simple_interest_golden() {
        assert_eq!(simple_interest(1000.0, 0.05, 3.0).unwrap(), 150.0);
        assert_eq!(simple_interest_total(1000.0, 0.05, 3.0).unwrap(), 1150.0);
    }

    #[test]
    fn margin_and_markup() {
        assert!(close(gross_margin_price(70.0, 0.30).unwrap(), 100.0, 1e-9));
        assert!(close(markup_price(80.0, 0.25).unwrap(), 100.0, 1e-9));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn depreciation_sln_golden() {
        assert_eq!(depreciation_sln(1000.0, 100.0, 5.0).unwrap(), 180.0);
    }

    #[test]
    fn depreciation_syd_golden() {
        assert!(close(
            depreciation_syd(1000.0, 100.0, 5.0, 1.0).unwrap(),
            300.0,
            1e-9
        ));
        assert!(close(
            depreciation_syd(1000.0, 100.0, 5.0, 5.0).unwrap(),
            60.0,
            1e-9
        ));
    }

    #[test]
    fn depreciation_syd_invalid_period() {
        // Blank period (UI maps blank -> 0) is invalid.
        assert!(depreciation_syd(1000.0, 100.0, 5.0, 0.0).is_err());
        // period > life is invalid.
        assert!(depreciation_syd(1000.0, 100.0, 5.0, 6.0).is_err());
        // Non-integer period is invalid.
        assert!(depreciation_syd(1000.0, 100.0, 5.0, 2.5).is_err());
        // Valid period still works.
        assert!(close(
            depreciation_syd(1000.0, 100.0, 5.0, 1.0).unwrap(),
            300.0,
            1e-9
        ));
    }

    #[test]
    fn depreciation_ddb_golden() {
        // cost 1000, salvage 100, life 5, rate = 0.4
        assert!(close(depreciation_ddb(1000.0, 100.0, 5.0, 1.0).unwrap(), 400.0, 1e-9));
        assert!(close(depreciation_ddb(1000.0, 100.0, 5.0, 2.0).unwrap(), 240.0, 1e-9));
        // Final period is clamped so book value never drops below salvage:
        // book at start of period 5 is 129.6, clamped depreciation = 29.6.
        assert!(close(depreciation_ddb(1000.0, 100.0, 5.0, 5.0).unwrap(), 29.6, 1e-9));
    }

    #[test]
    fn depreciation_ddb_period_zero() {
        // Blank period (UI maps blank -> 0) is now invalid.
        assert!(depreciation_ddb(1000.0, 100.0, 5.0, 0.0).is_err());
    }

    #[test]
    fn error_cases() {
        assert!(depreciation_sln(1000.0, 100.0, 0.0).is_err());
        assert!(gross_margin_price(70.0, 1.0).is_err());
        assert!(compound_n(0.0, 100.0, 0.05).is_err());
        assert!(compound_fv(f64::NAN, 0.05, 10.0).is_err());
        assert!(compound_pv(100.0, f64::INFINITY, 10.0).is_err());
        assert!(annuity_n(0.0, 100.0, 0.05).is_err());
        assert!(annuity_n(100.0, 100.0, 0.0).is_err());
        assert!(depreciation_ddb(1000.0, 100.0, 0.0, 1.0).is_err());
        assert!(depreciation_ddb(1000.0, 100.0, 5.0, -1.0).is_err());
        assert!(compound_rate(1000.0, 2000.0, 0.0).is_err());
    }
}
