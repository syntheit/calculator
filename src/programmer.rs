//! Integer expression evaluator for the calculator's *programmer mode*.
//!
//! This module is a small, self-contained arithmetic engine that evaluates an
//! integer expression whose numeric **literals** are written in a chosen input
//! base (binary / octal / decimal / hexadecimal), computes the result with
//! fixed-width **wrapping** semantics (8/16/32/64-bit), and formats a value back
//! out in any base. It is deliberately UI-agnostic: no GTK, no formatting glue,
//! just `&str` in and `i128` / `String` out.
//!
//! # Pipeline
//!
//! ```text
//! &str ──lex(base)──▶ Vec<Token> ──parse(width,signed)──▶ raw u128 ──mask──▶ i128
//! ```
//!
//! The lexer is base-aware: it consumes a maximal run of `[0-9A-Fa-f]` for a
//! literal and then validates every character against the active base, so `"FF"`
//! is a valid literal under `Hex` but a `Syntax` error under `Dec` (never a
//! panic). The parser is a classic precedence-climbing recursive descent; each
//! `parse_*` level calls *down* to the next tighter level and evaluates inline,
//! carrying `width` and `signed` so it can apply masking as it goes.
//!
//! # Precedence table (loosest ─▶ tightest)
//!
//! ```text
//!   level  operators        associativity   notes
//!   -----  ---------------  -------------   -------------------------------------
//!     1    |                left            bitwise OR                 (loosest)
//!     2    ^                left            bitwise XOR — see note below
//!     3    &                left            bitwise AND
//!     4    << >>            left            shifts
//!     5    + -              left            add / subtract
//!     6    * / %            left            multiply / div / remainder
//!     7    ~ (unary)        —               bitwise NOT
//!          - (unary)        —               negate                     (tightest)
//!     0    ( )              —               parentheses group anything
//! ```
//!
//! All binary operators are **left-associative**. This mirrors C's precedence,
//! with the deliberate consequence that `1 + 2 << 3` parses as `(1 + 2) << 3`
//! (`+` is tighter than `<<`) and evaluates to `24`, not `1 + (2 << 3)`.
//!
//! # Design decisions
//!
//! * **`^` is XOR, not power.** In an ordinary scientific calculator `^` often
//!   means exponentiation, but programmer mode follows C: `^` is the bitwise
//!   exclusive-OR operator. There is no exponentiation operator here.
//! * **No alphabetic operator names.** We support *only* the symbolic operators
//!   `& | ^ ~ << >>` (plus `+ - * / % ( )`). We do **not** accept `AND`, `OR`,
//!   `XOR`, `NOT`, etc., because hex literals are written with the letters
//!   `A`–`F`, so an alphabetic operator keyword would be ambiguous with (and
//!   collide against) a hex literal.
//! * **Wrapping, fixed-width arithmetic.** An internal `u128` accumulator is
//!   masked to `width.mask()` after every operation, so e.g. `0xFF + 1` at
//!   `W8` is `0` and `1 << 8` at `W8` is `0`.
//! * **Division / remainder honour `signed`.** `/` and `%` use C-like
//!   truncation-toward-zero then re-mask. When `signed`, both operands are
//!   reinterpreted as signed N-bit values first; when unsigned, the raw masked
//!   bit-patterns are used directly.
//! * **Binary formatting is zero-padded with no spaces.** `format` renders
//!   binary padded to the full `width.bits()` and inserts **no** separators or
//!   nibble grouping: `W8` value `10` renders as `"00001010"`, and `-1` at `W8`
//!   renders as `"11111111"` (exactly 8 characters). Hex and octal use minimal
//!   width with no padding.

use std::cell::Cell;

/// A numeric base for input literals and output formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// Binary, radix 2.
    Bin,
    /// Octal, radix 8.
    Oct,
    /// Decimal, radix 10.
    Dec,
    /// Hexadecimal, radix 16.
    Hex,
}

impl Base {
    /// The radix (numeric base) as an integer: 2, 8, 10 or 16.
    pub fn radix(self) -> u32 {
        match self {
            Base::Bin => 2,
            Base::Oct => 8,
            Base::Dec => 10,
            Base::Hex => 16,
        }
    }

    /// Whether `c` is a valid digit for this base (case-insensitive for hex).
    pub fn is_valid_digit(self, c: char) -> bool {
        c.is_digit(self.radix())
    }
}

/// A machine word width in bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    /// 8-bit word.
    W8,
    /// 16-bit word.
    W16,
    /// 32-bit word.
    W32,
    /// 64-bit word.
    W64,
}

impl Width {
    /// The number of bits in this width: 8, 16, 32 or 64.
    pub fn bits(self) -> u32 {
        match self {
            Width::W8 => 8,
            Width::W16 => 16,
            Width::W32 => 32,
            Width::W64 => 64,
        }
    }

    /// A low-bit mask covering this width, e.g. `W8 -> 0xFF`, `W64 -> u64::MAX`.
    pub fn mask(self) -> u128 {
        match self {
            Width::W8 => 0xFF,
            Width::W16 => 0xFFFF,
            Width::W32 => 0xFFFF_FFFF,
            Width::W64 => u64::MAX as u128,
        }
    }
}

/// Errors produced while evaluating a programmer-mode expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProgError {
    /// The input is not a well-formed expression for the active base.
    #[error("Bad expression")]
    Syntax,
    /// A `/` or `%` had a right-hand side equal to zero.
    #[error("Can't divide by 0")]
    DivideByZero,
    /// A value did not fit (reserved; wrapping semantics mean this is rare).
    #[error("Overflow")]
    Overflow,
}

/// A lexical token in a programmer-mode expression.
///
/// Note: only the *symbolic* operators are represented here — there are
/// deliberately no alphabetic operator tokens, because `A`–`F` are hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// A numeric literal, already parsed in the active base.
    Number(u128),
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `&`
    Amp,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `(`
    LParen,
    /// `)`
    RParen,
}

/// Tokenize `input` interpreting numeric literals in `base`.
///
/// Whitespace between tokens is skipped. A maximal run of `[0-9A-Fa-f]` is taken
/// as one literal and validated against `base` (so `'F'` under `Dec` is rejected
/// as `Syntax`, never panicking). Any character outside the grammar, or a lone
/// `<` / `>` that is not doubled, yields `ProgError::Syntax`.
fn lex(input: &str, base: Base) -> Result<Vec<Token>, ProgError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '&' => {
                tokens.push(Token::Amp);
                i += 1;
            }
            '|' => {
                tokens.push(Token::Pipe);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '~' => {
                tokens.push(Token::Tilde);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            // `<<` / `>>` are two-char tokens; a lone `<` or `>` is a Syntax error.
            '<' => {
                if chars.get(i + 1) == Some(&'<') {
                    tokens.push(Token::Shl);
                    i += 2;
                } else {
                    return Err(ProgError::Syntax);
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'>') {
                    tokens.push(Token::Shr);
                    i += 2;
                } else {
                    return Err(ProgError::Syntax);
                }
            }
            // A digit run: consume the maximal `[0-9A-Fa-f]` span, then validate
            // it against the active base. This makes "FF" under Dec a clean
            // Syntax error rather than a panic.
            '0'..='9' | 'A'..='F' | 'a'..='f' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let run = &chars[start..i];
                let mut value: u128 = 0;
                for &ch in run {
                    let digit = match ch.to_digit(base.radix()) {
                        Some(d) => d,
                        None => return Err(ProgError::Syntax),
                    };
                    value = value
                        .wrapping_mul(base.radix() as u128)
                        .wrapping_add(digit as u128);
                }
                tokens.push(Token::Number(value));
            }
            _ => return Err(ProgError::Syntax),
        }
    }

    Ok(tokens)
}

/// Maximum recursive-descent depth. Deeply nested input like `(((…1…)))`, a
/// run of unary `~` (`~~~…`), or a run of unary `-` (`----…`) recurses ~one
/// guard level per nesting level: the [`DepthGuard`] is applied at
/// [`Parser::parse_unary`] (the `~` / unary `-` chain) and
/// [`Parser::parse_atom`] (the `(` … `)` descent back into `parse_or`), each
/// entered ~once per real nesting level. Past this cap we return
/// [`ProgError::Syntax`] instead of overflowing the stack.
const MAX_DEPTH: usize = 256;

/// RAII guard that increments the parser's recursion depth on construction and
/// decrements it on drop (panic-safe, and runs on every `?` early-return path).
/// Holds a shared borrow of a `Cell`, so the parse methods can keep `&mut self`.
struct DepthGuard<'a>(&'a Cell<usize>);

impl<'a> DepthGuard<'a> {
    fn new(d: &'a Cell<usize>) -> Result<Self, ProgError> {
        let n = d.get() + 1;
        if n > MAX_DEPTH {
            return Err(ProgError::Syntax);
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

/// A precedence-climbing recursive-descent parser that evaluates inline.
///
/// Each `parse_*` method returns the running `u128` accumulator already masked
/// to `width`. Carrying `width` and `signed` lets division / shifts apply the
/// correct sign interpretation without a separate AST pass.
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    width: Width,
    signed: bool,
    /// Reference to a caller-owned recursion counter. Held as a reference (not
    /// an owned `Cell`) so a [`DepthGuard`] can borrow it without also borrowing
    /// `self`, leaving the parse methods free to take `&mut self`.
    depth: &'a Cell<usize>,
}

impl<'a> Parser<'a> {
    /// Look at the current token without consuming it.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// Consume and return the current token, advancing only if one is present.
    fn bump(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// Parse a full expression and require that all tokens were consumed.
    fn parse(&mut self) -> Result<u128, ProgError> {
        let value = self.parse_or()?;
        if self.pos != self.tokens.len() {
            return Err(ProgError::Syntax);
        }
        Ok(value)
    }

    /// Level 1: bitwise OR (`|`), loosest binary precedence.
    fn parse_or(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_xor()?;
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.bump();
            let rhs = self.parse_xor()?;
            lhs = (lhs | rhs) & self.width.mask();
        }
        Ok(lhs)
    }

    /// Level 2: bitwise XOR (`^`). In programmer mode `^` is XOR, not power.
    fn parse_xor(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Caret)) {
            self.bump();
            let rhs = self.parse_and()?;
            lhs = (lhs ^ rhs) & self.width.mask();
        }
        Ok(lhs)
    }

    /// Level 3: bitwise AND (`&`).
    fn parse_and(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_shift()?;
        while matches!(self.peek(), Some(Token::Amp)) {
            self.bump();
            let rhs = self.parse_shift()?;
            lhs = (lhs & rhs) & self.width.mask();
        }
        Ok(lhs)
    }

    /// Level 4: shifts (`<<`, `>>`).
    fn parse_shift(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_add()?;
        loop {
            match self.peek() {
                Some(Token::Shl) => {
                    self.bump();
                    let rhs = self.parse_add()?;
                    lhs = self.shift_left(lhs, rhs);
                }
                Some(Token::Shr) => {
                    self.bump();
                    let rhs = self.parse_add()?;
                    lhs = self.shift_right(lhs, rhs);
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Level 5: addition / subtraction (`+`, `-`).
    fn parse_add(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_mul()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.bump();
                    let rhs = self.parse_mul()?;
                    lhs = lhs.wrapping_add(rhs) & self.width.mask();
                }
                Some(Token::Minus) => {
                    self.bump();
                    let rhs = self.parse_mul()?;
                    lhs = lhs.wrapping_sub(rhs) & self.width.mask();
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Level 6: multiply / divide / remainder (`*`, `/`, `%`).
    fn parse_mul(&mut self) -> Result<u128, ProgError> {
        let mut lhs = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.bump();
                    let rhs = self.parse_unary()?;
                    lhs = lhs.wrapping_mul(rhs) & self.width.mask();
                }
                Some(Token::Slash) => {
                    self.bump();
                    let rhs = self.parse_unary()?;
                    lhs = self.divide(lhs, rhs)?;
                }
                Some(Token::Percent) => {
                    self.bump();
                    let rhs = self.parse_unary()?;
                    lhs = self.remainder(lhs, rhs)?;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Level 7: unary `~` (bitwise NOT) and unary `-` (negate).
    fn parse_unary(&mut self) -> Result<u128, ProgError> {
        let _guard = DepthGuard::new(self.depth)?;
        match self.peek() {
            Some(Token::Tilde) => {
                self.bump();
                let v = self.parse_unary()?;
                Ok((!v) & self.width.mask())
            }
            Some(Token::Minus) => {
                self.bump();
                let v = self.parse_unary()?;
                Ok(v.wrapping_neg() & self.width.mask())
            }
            _ => self.parse_atom(),
        }
    }

    /// Level 0: a literal or a parenthesized sub-expression.
    fn parse_atom(&mut self) -> Result<u128, ProgError> {
        let _guard = DepthGuard::new(self.depth)?;
        match self.bump() {
            Some(&Token::Number(n)) => Ok(n & self.width.mask()),
            Some(&Token::LParen) => {
                let inner = self.parse_or()?;
                match self.bump() {
                    Some(&Token::RParen) => Ok(inner),
                    _ => Err(ProgError::Syntax),
                }
            }
            _ => Err(ProgError::Syntax),
        }
    }

    /// `a << b` with `b` as the unsigned bit-pattern of the RHS; a shift amount
    /// that is `>= width.bits()` (or `>= 128`) yields 0.
    fn shift_left(&self, a: u128, b: u128) -> u128 {
        let amount = masked(b as i128, self.width, false) as u128;
        if amount >= self.width.bits() as u128 || amount >= 128 {
            0
        } else {
            (a << amount) & self.width.mask()
        }
    }

    /// `a >> b`. Arithmetic (sign-extending) when `signed`, else logical.
    fn shift_right(&self, a: u128, b: u128) -> u128 {
        let amount = masked(b as i128, self.width, false) as u128;
        if self.signed {
            // Arithmetic shift: sign-extend. Clamp the shift so an out-of-range
            // amount still fills correctly with the sign bit.
            let signed_a = masked(a as i128, self.width, true);
            let bits = self.width.bits();
            let shift = if amount >= bits as u128 {
                bits - 1
            } else {
                amount as u32
            };
            let shifted = signed_a >> shift;
            (shifted as u128) & self.width.mask()
        } else if amount >= self.width.bits() as u128 || amount >= 128 {
            0
        } else {
            (a >> amount) & self.width.mask()
        }
    }

    /// Integer division truncating toward zero (C-like). Honours `signed`:
    /// when unsigned it divides the raw masked bit-patterns, when signed it
    /// sign-extends both operands first.
    fn divide(&self, a: u128, b: u128) -> Result<u128, ProgError> {
        if b == 0 {
            return Err(ProgError::DivideByZero);
        }
        if self.signed {
            let sa = masked(a as i128, self.width, true);
            let sb = masked(b as i128, self.width, true);
            Ok((sa / sb) as u128 & self.width.mask())
        } else {
            Ok((a / b) & self.width.mask())
        }
    }

    /// Remainder with the C-like sign-of-dividend convention. Honours `signed`:
    /// when unsigned it uses the raw masked bit-patterns, when signed it
    /// sign-extends both operands first.
    fn remainder(&self, a: u128, b: u128) -> Result<u128, ProgError> {
        if b == 0 {
            return Err(ProgError::DivideByZero);
        }
        if self.signed {
            let sa = masked(a as i128, self.width, true);
            let sb = masked(b as i128, self.width, true);
            Ok((sa % sb) as u128 & self.width.mask())
        } else {
            Ok((a % b) & self.width.mask())
        }
    }
}

/// Evaluate `expr`, whose literals are written in `base`, with fixed-`width`
/// wrapping arithmetic; the result is interpreted `signed` for the final value.
pub fn evaluate(expr: &str, base: Base, width: Width, signed: bool) -> Result<i128, ProgError> {
    let tokens = lex(expr, base)?;
    if tokens.is_empty() {
        return Err(ProgError::Syntax);
    }
    let depth = Cell::new(0);
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        width,
        signed,
        depth: &depth,
    };
    let raw = parser.parse()?;
    Ok(masked(raw as i128, width, signed))
}

/// Format `value` in `base`, at `width`, with `signed` interpretation.
///
/// Decimal honours `signed` (rendering negatives with a leading `-`). Hex, octal
/// and binary always render the unsigned masked bit-pattern. Binary is
/// zero-padded to `width.bits()` with no spaces; hex/octal use minimal width.
pub fn format(value: i128, base: Base, width: Width, signed: bool) -> String {
    let unsigned = (value as u128) & width.mask();
    match base {
        Base::Dec => {
            if signed {
                masked(value, width, true).to_string()
            } else {
                unsigned.to_string()
            }
        }
        Base::Hex => format!("{unsigned:X}"),
        Base::Oct => format!("{unsigned:o}"),
        // Binary: zero-pad to full width, no separators / nibble grouping.
        Base::Bin => format!("{unsigned:0width$b}", width = width.bits() as usize),
    }
}

/// Mask `value` to `width`, sign-extending into a negative `i128` when `signed`
/// and the top bit of the masked pattern is set.
pub fn masked(value: i128, width: Width, signed: bool) -> i128 {
    let u = (value as u128) & width.mask();
    if signed && (u >> (width.bits() - 1)) & 1 == 1 {
        (u as i128) - (1i128 << width.bits())
    } else {
        u as i128
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_radix() {
        assert_eq!(Base::Bin.radix(), 2);
        assert_eq!(Base::Oct.radix(), 8);
        assert_eq!(Base::Dec.radix(), 10);
        assert_eq!(Base::Hex.radix(), 16);
    }

    #[test]
    fn base_is_valid_digit() {
        assert!(Base::Hex.is_valid_digit('F'));
        assert!(Base::Hex.is_valid_digit('f'));
        assert!(!Base::Dec.is_valid_digit('F'));
        assert!(!Base::Oct.is_valid_digit('9'));
        assert!(!Base::Bin.is_valid_digit('2'));
        assert!(Base::Oct.is_valid_digit('7'));
        assert!(Base::Bin.is_valid_digit('1'));
    }

    #[test]
    fn width_bits() {
        assert_eq!(Width::W8.bits(), 8);
        assert_eq!(Width::W16.bits(), 16);
        assert_eq!(Width::W32.bits(), 32);
        assert_eq!(Width::W64.bits(), 64);
    }

    #[test]
    fn width_mask() {
        assert_eq!(Width::W8.mask(), 0xFF);
        assert_eq!(Width::W16.mask(), 0xFFFF);
        assert_eq!(Width::W32.mask(), 0xFFFF_FFFF);
        assert_eq!(Width::W64.mask(), u64::MAX as u128);
    }

    #[test]
    fn bitwise_and() {
        assert_eq!(evaluate("FF & 0F", Base::Hex, Width::W8, false), Ok(15));
    }

    #[test]
    fn bitwise_or() {
        assert_eq!(evaluate("F0 | 0F", Base::Hex, Width::W8, false), Ok(255));
    }

    #[test]
    fn bitwise_xor() {
        assert_eq!(evaluate("1010 ^ 0110", Base::Bin, Width::W8, false), Ok(12));
    }

    #[test]
    fn bitwise_not_unsigned() {
        assert_eq!(evaluate("~0", Base::Dec, Width::W8, false), Ok(255));
    }

    #[test]
    fn bitwise_not_signed() {
        assert_eq!(evaluate("~0", Base::Dec, Width::W8, true), Ok(-1));
    }

    #[test]
    fn shift_left_basic() {
        assert_eq!(evaluate("5 << 2", Base::Dec, Width::W8, false), Ok(20));
    }

    #[test]
    fn shift_right_basic() {
        assert_eq!(evaluate("20 >> 2", Base::Dec, Width::W8, false), Ok(5));
    }

    #[test]
    fn format_hex_signed_neg() {
        assert_eq!(format(-1, Base::Hex, Width::W8, true), "FF");
    }

    #[test]
    fn format_bin_signed_neg() {
        assert_eq!(format(-1, Base::Bin, Width::W8, true), "11111111");
    }

    #[test]
    fn format_dec_signed_neg() {
        assert_eq!(format(-1, Base::Dec, Width::W8, true), "-1");
    }

    #[test]
    fn format_dec_unsigned() {
        assert_eq!(format(255, Base::Dec, Width::W8, false), "255");
    }

    #[test]
    fn remainder_basic() {
        assert_eq!(evaluate("10 % 3", Base::Dec, Width::W8, false), Ok(1));
    }

    #[test]
    fn divide_truncates() {
        assert_eq!(evaluate("7 / 2", Base::Dec, Width::W8, false), Ok(3));
    }

    #[test]
    fn add_wraps() {
        assert_eq!(evaluate("FF + 1", Base::Hex, Width::W8, false), Ok(0));
    }

    #[test]
    fn shift_left_out_of_range_is_zero() {
        assert_eq!(evaluate("1 << 8", Base::Dec, Width::W8, false), Ok(0));
    }

    #[test]
    fn hex_literal_fits_wider_width() {
        assert_eq!(evaluate("FF", Base::Hex, Width::W16, false), Ok(255));
    }

    #[test]
    fn oct_literal() {
        assert_eq!(evaluate("777", Base::Oct, Width::W16, false), Ok(511));
    }

    #[test]
    fn bin_literal() {
        assert_eq!(evaluate("1010", Base::Bin, Width::W8, false), Ok(10));
    }

    #[test]
    fn hex_digits_invalid_under_dec() {
        assert_eq!(
            evaluate("FF", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn precedence_add_tighter_than_shift() {
        assert_eq!(evaluate("1 + 2 << 3", Base::Dec, Width::W8, false), Ok(24));
    }

    #[test]
    fn multiply_hex() {
        assert_eq!(evaluate("10 * 10", Base::Hex, Width::W16, false), Ok(256));
    }

    #[test]
    fn unsigned_divide_no_sign_extend() {
        assert_eq!(evaluate("FF / 2", Base::Hex, Width::W8, false), Ok(127));
    }

    #[test]
    fn unsigned_remainder_no_sign_extend() {
        assert_eq!(evaluate("FF % 2", Base::Hex, Width::W8, false), Ok(1));
    }

    #[test]
    fn signed_divide_sign_extends() {
        // FF as signed W8 == -1, and -1 / 2 truncates toward zero to 0.
        assert_eq!(evaluate("FF / 2", Base::Hex, Width::W8, true), Ok(0));
    }

    #[test]
    fn signed_divide_truncates_toward_zero() {
        // -7 / 2 == -3.5 -> -3 (trunc toward zero).
        assert_eq!(evaluate("-7 / 2", Base::Dec, Width::W8, true), Ok(-3));
    }

    #[test]
    fn unsigned_divide_by_zero() {
        assert_eq!(
            evaluate("8 / 0", Base::Dec, Width::W8, false),
            Err(ProgError::DivideByZero)
        );
    }

    #[test]
    fn signed_divide_by_zero() {
        assert_eq!(
            evaluate("8 / 0", Base::Dec, Width::W8, true),
            Err(ProgError::DivideByZero)
        );
    }

    #[test]
    fn divide_by_zero() {
        assert_eq!(
            evaluate("5 / 0", Base::Dec, Width::W8, false),
            Err(ProgError::DivideByZero)
        );
    }

    #[test]
    fn remainder_by_zero() {
        assert_eq!(
            evaluate("5 % 0", Base::Dec, Width::W8, false),
            Err(ProgError::DivideByZero)
        );
    }

    #[test]
    fn masked_unsigned_neg_one() {
        assert_eq!(masked(-1, Width::W8, false), 255);
    }

    #[test]
    fn masked_signed_neg_one() {
        assert_eq!(masked(-1, Width::W8, true), -1);
    }

    #[test]
    fn masked_wraps_256() {
        assert_eq!(masked(256, Width::W8, false), 0);
    }

    #[test]
    fn parentheses_group() {
        assert_eq!(evaluate("(1 + 2) * 3", Base::Dec, Width::W8, false), Ok(9));
    }

    #[test]
    fn unary_minus_signed() {
        assert_eq!(evaluate("-1", Base::Dec, Width::W8, true), Ok(-1));
    }

    #[test]
    fn unary_minus_unsigned() {
        assert_eq!(evaluate("-1", Base::Dec, Width::W8, false), Ok(255));
    }

    #[test]
    fn whitespace_tolerance() {
        assert_eq!(
            evaluate("  5   +   3  ", Base::Dec, Width::W8, false),
            Ok(8)
        );
        assert_eq!(
            evaluate("\t5\n+\r3", Base::Dec, Width::W8, false),
            Ok(8)
        );
    }

    #[test]
    fn lone_lt_is_syntax() {
        assert_eq!(
            evaluate("1 < 2", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn lone_gt_is_syntax() {
        assert_eq!(
            evaluate("1 > 2", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn empty_is_syntax() {
        assert_eq!(
            evaluate("", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn garbage_is_syntax() {
        assert_eq!(
            evaluate("@", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn trailing_operator_is_syntax() {
        assert_eq!(
            evaluate("5 +", Base::Dec, Width::W8, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn format_bin_zero_padded() {
        assert_eq!(format(10, Base::Bin, Width::W8, false), "00001010");
    }

    #[test]
    fn format_hex_minimal() {
        assert_eq!(format(15, Base::Hex, Width::W8, false), "F");
    }

    #[test]
    fn format_oct_minimal_neg() {
        assert_eq!(format(-1, Base::Oct, Width::W8, true), "377");
    }

    #[test]
    fn deeply_nested_parens_is_syntax_not_crash() {
        // A pathological run of `(` (keyboard auto-repeat) must hit the
        // recursion-depth cap and return a clean Syntax error, never overflow
        // the stack. The unterminated parens are also a syntax error, but the
        // point is that we return rather than SIGABRT.
        let s = "(".repeat(5000);
        assert_eq!(
            evaluate(&s, Base::Dec, Width::W32, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn deeply_nested_tilde_is_syntax_not_crash() {
        let s = "~".repeat(5000);
        assert_eq!(
            evaluate(&s, Base::Dec, Width::W32, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn deeply_nested_minus_is_syntax_not_crash() {
        let s = "-".repeat(5000);
        assert_eq!(
            evaluate(&s, Base::Dec, Width::W32, false),
            Err(ProgError::Syntax)
        );
    }

    #[test]
    fn moderately_nested_parens_still_evaluate() {
        // Nesting well under the cap must still work: the guard must not break
        // ordinary parenthesized input.
        let s = "(".repeat(50) + "1" + &")".repeat(50);
        assert_eq!(evaluate(&s, Base::Dec, Width::W32, false), Ok(1));
    }
}

