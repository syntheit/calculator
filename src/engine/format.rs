//! Number formatting for display.
//!
//! The engine works in `f64`. This module renders a result the way Google
//! Calculator / AOSP ExactCalculator does for the common cases:
//!
//! * thousands grouping (`2025 → "2,025"`, `1000000 → "1,000,000"`),
//! * trailing-zero trimming (`2.50 → "2.5"`, `3.0 → "3"`),
//! * rounding to ~12 significant figures so `f64` identity noise disappears
//!   (`cos(π/3)` shows `"0.5"`, not `"0.4999999999999"`),
//! * scientific notation for very large / very small magnitudes,
//! * an ellipsis for values that do not terminate within the shown precision
//!   (`1/3 → "0.33333…"`).
//!
//! COSMETIC PRECISION CAVEAT: we use `f64`, not constructive reals. The ellipsis
//! means "there are more digits than shown at this fixed precision", not "this
//! number is provably irrational". `0.1 + 0.2` displays as `"0.3"` because we
//! round to 12 significant figures before formatting; that is intentional.

/// The thousands-group separator (en-US default). Exposed so a future locale
/// layer can swap it without touching the formatting logic.
pub const GROUP_SEPARATOR: char = ',';
/// The decimal separator (en-US default).
pub const DECIMAL_SEPARATOR: char = '.';

/// Locale governing the group + decimal separators used when rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumLocale {
    /// en-US: thousands group ",", decimal ".".
    EnUs,
    /// es-AR (Argentina): thousands group ".", decimal ",".
    EsAr,
}

impl NumLocale {
    /// The thousands-group separator for this locale.
    pub fn group(self) -> char {
        match self {
            NumLocale::EnUs => ',',
            NumLocale::EsAr => '.',
        }
    }

    /// The decimal separator for this locale.
    pub fn decimal(self) -> char {
        match self {
            NumLocale::EnUs => '.',
            NumLocale::EsAr => ',',
        }
    }
}

/// Significant figures we keep before formatting. 12 is enough to hide f64
/// identity artifacts while staying well inside f64's ~15–17 digit budget.
const SIG_FIGS: i32 = 12;

/// Format `value` for the calculator display using en-US defaults.
pub fn format_result(value: f64) -> String {
    format_with(value, GROUP_SEPARATOR, DECIMAL_SEPARATOR)
}

/// Format `value` for display using the given locale's separators. Delegates to
/// the shared `format_with` so grouping, trailing-zero trim, E-notation and the
/// non-terminating ellipsis all behave identically across locales — only the
/// group + decimal glyphs differ.
pub fn format_result_locale(value: f64, locale: NumLocale) -> String {
    format_with(value, locale.group(), locale.decimal())
}

/// Format with explicit separators (for locale experiments / tests).
pub fn format_with(value: f64, group: char, decimal: char) -> String {
    // Non-finite values should have been mapped to errors upstream; be defensive.
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-∞".to_string() } else { "∞".to_string() };
    }
    if value == 0.0 {
        return "0".to_string();
    }

    // Round once: reused for the fixed-vs-scientific threshold AND for the
    // fixed formatter below, so `round_sig` (a log10 + powi + round) runs a
    // single time per fixed-path result instead of twice.
    //
    // Decide fixed-vs-scientific against the ROUNDED magnitude, not the raw
    // value: e.g. 999999999999999.0 is < 1e15 but rounds up to 1e15 at 12 sig
    // figs, and must go to E-notation rather than print 16 grouped digits.
    let (rounded, non_terminating) = round_sig(value);
    let abs = rounded.abs();
    // Thresholds: very large or very small magnitudes go to E-notation, where
    // fixed formatting would otherwise emit a wall of zeros.
    if !(1e-6..1e15).contains(&abs) {
        return format_scientific(value, decimal);
    }

    format_fixed(rounded, non_terminating, group, decimal)
}

/// Round `value` to [`SIG_FIGS`] significant figures. Returns the rounded value
/// and whether rounding actually changed it (a cheap proxy for "the exact value
/// has more digits than we show" → show an ellipsis).
fn round_sig(value: f64) -> (f64, bool) {
    if value == 0.0 {
        return (0.0, false);
    }
    let digits = value.abs().log10().floor() as i32;
    // Number of decimal places to keep so that we retain SIG_FIGS sig-figs.
    let decimals = SIG_FIGS - 1 - digits;
    let factor = 10f64.powi(decimals);
    let rounded = (value * factor).round() / factor;
    // "Non-terminating" heuristic: the rounded value differs from the raw value
    // by more than a rounding-noise epsilon at this scale.
    let changed = (rounded - value).abs() > (value.abs() * 1e-15).max(f64::MIN_POSITIVE);
    (rounded, changed)
}

/// Fixed-point formatting with grouping, trailing-zero trim and ellipsis.
/// `rounded` is the value already put through [`round_sig`] by the caller, and
/// `non_terminating` is that same rounding's "dropped-detail" flag — so the
/// rounding work is not repeated here.
fn format_fixed(rounded: f64, non_terminating: bool, group: char, decimal: char) -> String {
    let negative = rounded < 0.0;
    let abs = rounded.abs();

    // Render with generous precision, then trim. We keep up to (SIG_FIGS)
    // fractional digits which is always enough for the rounded value.
    let digits_after = {
        let int_digits = if abs >= 1.0 {
            abs.log10().floor() as i32 + 1
        } else {
            0
        };
        (SIG_FIGS - int_digits).clamp(0, 15) as usize
    };
    let mut s = format!("{:.*}", digits_after, abs);

    // Split integer / fractional.
    let (int_part, frac_part) = match s.find('.') {
        Some(dot) => {
            let (i, f) = s.split_at(dot);
            (i.to_string(), f[1..].to_string())
        }
        None => (s.clone(), String::new()),
    };

    let grouped_int = group_integer(&int_part, group);

    // Trim trailing zeros from the fractional part.
    let trimmed_frac = frac_part.trim_end_matches('0');

    s = if trimmed_frac.is_empty() {
        grouped_int
    } else {
        format!("{}{}{}", grouped_int, decimal, trimmed_frac)
    };

    // Append the ellipsis only when the raw value truly does not terminate at
    // our precision AND we actually dropped fractional detail (i.e. there is a
    // fractional part to be "more of").
    if non_terminating && !trimmed_frac.is_empty() {
        s.push('\u{2026}');
    }

    if negative {
        format!("-{}", s)
    } else {
        s
    }
}

/// Group the digits of a (non-negative) integer string in threes.
fn group_integer(int_part: &str, group: char) -> String {
    let bytes = int_part.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, ch) in int_part.chars().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(group);
        }
        out.push(ch);
    }
    let _ = bytes; // silence unused in some cfgs
    out
}

/// Scientific notation: `mantissa E exp`, e.g. `1.23E5`. The mantissa and
/// exponent are derived from Rust's own `{:e}` exponent formatter — which never
/// underflows the way `10f64.powi(exp)` does for subnormals near 1e-323 — rather
/// than from `log10().floor()` + a `powi` division. The `{:e}` mantissa is
/// already in `[1, 10)` and its exponent is already the base-10 exponent, so we
/// simply run the mantissa through the same round / renormalize / trailing-zero
/// trim pipeline used before; behavior for normal values is unchanged.
fn format_scientific(value: f64, decimal: char) -> String {
    let negative = value < 0.0;
    let abs = value.abs();
    // `{:e}` on a positive finite f64 yields e.g. "1.234e-323" / "1e5" and never
    // underflows; split on the single 'e' to recover mantissa and exponent.
    let (mantissa_str, exp_str) = format!("{:e}", abs)
        .split_once('e')
        .map(|(m, e)| (m.to_string(), e.to_string()))
        .expect("`{:e}` of a finite f64 always contains one 'e'");
    let mantissa: f64 = mantissa_str
        .parse()
        .expect("`{:e}` mantissa is a valid f64 in [1,10)");
    let mut exp: i32 = exp_str
        .parse()
        .expect("`{:e}` exponent is a valid i32");
    // Round the mantissa to SIG_FIGS and trim.
    let (mut m_rounded, _) = round_sig(mantissa);
    // Rounding can push the mantissa to exactly 10.0 (e.g. 9.9999999999995e17
    // rounds up); renormalize back into [1,10) and bump the exponent.
    if m_rounded >= 10.0 {
        m_rounded /= 10.0;
        exp += 1;
    }
    let mut m = format!("{:.*}", (SIG_FIGS - 1) as usize, m_rounded);
    if let Some(dot) = m.find('.') {
        let (i, f) = m.split_at(dot);
        let trimmed = f[1..].trim_end_matches('0');
        m = if trimmed.is_empty() {
            i.to_string()
        } else {
            format!("{}{}{}", i, decimal, trimmed)
        };
    }
    let sign = if negative { "-" } else { "" };
    format!("{}{}E{}", sign, m, exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_grouping() {
        assert_eq!(format_result(2025.0), "2,025");
        assert_eq!(format_result(1_000_000.0), "1,000,000");
        assert_eq!(format_result(999.0), "999");
        assert_eq!(format_result(12_345.0), "12,345");
    }

    #[test]
    fn trailing_zero_trim() {
        assert_eq!(format_result(2.5), "2.5");
        assert_eq!(format_result(2.50), "2.5");
        assert_eq!(format_result(3.0), "3");
    }

    #[test]
    fn negative_values() {
        assert_eq!(format_result(-2025.0), "-2,025");
        assert_eq!(format_result(-3.0), "-3");
    }

    #[test]
    fn zero() {
        assert_eq!(format_result(0.0), "0");
    }

    #[test]
    fn identity_noise_rounded_away() {
        // cos(π/3) = 0.4999999999999999 in f64 → should show "0.5".
        let v = (std::f64::consts::PI / 3.0).cos();
        assert_eq!(format_result(v), "0.5");
    }

    #[test]
    fn one_third_has_ellipsis() {
        let s = format_result(1.0 / 3.0);
        assert!(s.starts_with("0.3333"), "got {s}");
        assert!(s.ends_with('\u{2026}'), "got {s}");
    }

    #[test]
    fn large_magnitude_scientific() {
        let s = format_result(1.23e20);
        assert!(s.contains('E'), "got {s}");
    }

    #[test]
    fn small_magnitude_scientific() {
        let s = format_result(1.23e-9);
        assert!(s.contains('E'), "got {s}");
    }

    #[test]
    fn scientific_shape() {
        assert_eq!(format_result(123_000.0), "123,000");
        // 1.23e5 falls below the 1e15 threshold, so it is plain, not E.
        // Force a large one:
        assert_eq!(format_result(1.23e18), "1.23E18");
    }

    #[test]
    fn scientific_mantissa_renormalizes() {
        // Mantissa rounds up to 10.0 → must renormalize to 1E18, not "10E17".
        assert_eq!(format_result(9.9999999999995e17), "1E18");
    }

    #[test]
    fn large_value_still_formats() {
        assert_eq!(format_result(1.23e18), "1.23E18");
    }

    #[test]
    fn fixed_boundary_goes_scientific() {
        // 999999999999999 rounds to 1e15 at 12 sig figs → E-notation, "1E15".
        assert_eq!(format_result(999999999999999.0), "1E15");
    }

    #[test]
    fn terminating_decimal_no_ellipsis() {
        assert_eq!(format_result(0.5), "0.5");
        assert_eq!(format_result(0.25), "0.25");
    }

    #[test]
    fn es_ar_formatting() {
        assert_eq!(format_result_locale(2025.0, NumLocale::EsAr), "2.025");
        assert_eq!(format_result_locale(2.5, NumLocale::EsAr), "2,5");
        assert_eq!(format_result_locale(1_000_000.0, NumLocale::EsAr), "1.000.000");
        assert_eq!(format_result_locale(1234.5, NumLocale::EsAr), "1.234,5");
    }

    #[test]
    fn en_us_formatting() {
        assert_eq!(format_result_locale(2025.0, NumLocale::EnUs), "2,025");
        assert_eq!(format_result_locale(2.5, NumLocale::EnUs), "2.5");
    }

    #[test]
    fn locale_scientific_and_ellipsis() {
        assert_eq!(format_result_locale(1.23e18, NumLocale::EsAr), "1,23E18");
        assert_eq!(format_result_locale(1.23e18, NumLocale::EnUs), "1.23E18");
        let s = format_result_locale(1.0 / 3.0, NumLocale::EsAr);
        assert!(s.starts_with("0,3333"), "got {s}");
        assert!(s.ends_with('\u{2026}'), "got {s}");
    }

    #[test]
    fn locale_accessors() {
        assert_eq!(NumLocale::EnUs.group(), ',');
        assert_eq!(NumLocale::EnUs.decimal(), '.');
        assert_eq!(NumLocale::EsAr.group(), '.');
        assert_eq!(NumLocale::EsAr.decimal(), ',');
    }

    #[test]
    fn subnormal_formats_without_nan() {
        let s = format_result(1e-323); // a subnormal
        assert!(!s.contains("NaN"), "got {s}");
        assert!(s.contains('E'), "got {s}");
        let parsed = s.replace('E', "e").parse::<f64>();
        // Extract just the numeric part before asserting finiteness. `s` has no
        // grouping (scientific path), so the whole string parses after E→e.
        let n = parsed.expect("should parse");
        assert!(n.is_finite(), "got {s}");
        assert!(n > 0.0, "got {s}");
        // Exponent is very negative (around -323/-324); be lenient.
        assert!(s.contains("E-32"), "got {s}");
    }

    #[test]
    fn subnormal_min_positive() {
        // f64::MIN_POSITIVE (~2.2e-308) is a NORMAL number.
        let s1 = format_result(f64::MIN_POSITIVE);
        assert!(!s1.contains("NaN"), "got {s1}");
        assert!(s1.contains('E'), "got {s1}");
        // The smallest subnormal (~5e-324).
        let s2 = format_result(f64::from_bits(1));
        assert!(!s2.contains("NaN"), "got {s2}");
        assert!(s2.contains('E'), "got {s2}");
    }
}

