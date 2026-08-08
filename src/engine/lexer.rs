//! Tokenizer for the canonical expression string, plus synthetic
//! implicit-multiplication insertion.
//!
//! The lexer consumes the *canonical* form the state machine builds (ASCII
//! operators `+ - * / ^`, `%`, `!`, parens, digits and `.`, the constant glyphs
//! `π`/`e`, the prefix `√`, and function-name-plus-open-paren tokens such as
//! `sin(`). It emits a flat [`Token`] stream in which a synthetic
//! [`Token::Star`] has been inserted everywhere juxtaposition means "multiply"
//! (`2π`, `2(3)`, `(1+2)(3)`, `2√9`, `2sin(0)`, …).
//!
//! Keeping implicit multiplication in the lexer keeps the parser a plain
//! precedence grammar with no special juxtaposition rules.

use crate::engine::EvalError;

/// A lexical token in the canonical expression grammar.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// A numeric literal (already parsed to `f64`). Multi-digit runs and an
    /// optional single decimal point are collapsed into one token.
    Number(f64),
    /// `+`
    Plus,
    /// `-` (binary or unary; the parser decides from position).
    Minus,
    /// `*` — including synthetic implicit-multiplication stars.
    Star,
    /// `/`
    Slash,
    /// `^` (right-associative power).
    Caret,
    /// `%` (Google percent; resolved in the evaluator).
    Percent,
    /// `!` (postfix factorial).
    Bang,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `√` (prefix square root).
    Sqrt,
    /// The constant π.
    Pi,
    /// The constant e (Euler's number).
    E,
    /// A function name whose argument is parenthesised. The open paren that
    /// follows in the source is emitted as a separate [`Token::LParen`], so a
    /// function token is always followed by an `LParen` in the stream.
    Func(FuncName),
}

/// The named unary functions the engine understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuncName {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Ln,
    Log,
    Exp,
}

/// Function names ordered longest-first so a greedy prefix match never picks a
/// shorter spelling by mistake (none currently share a prefix, but keeping the
/// order explicit is cheap insurance).
const FUNCS: &[(&str, FuncName)] = &[
    ("asin", FuncName::Asin),
    ("acos", FuncName::Acos),
    ("atan", FuncName::Atan),
    ("sin", FuncName::Sin),
    ("cos", FuncName::Cos),
    ("tan", FuncName::Tan),
    ("log", FuncName::Log),
    ("ln", FuncName::Ln),
    ("exp", FuncName::Exp),
];

/// Tokenize the canonical string and insert implicit-multiplication stars.
///
/// Returns [`EvalError::Syntax`] on any character that is not part of the
/// canonical alphabet.
pub fn tokenize(input: &str) -> Result<Vec<Token>, EvalError> {
    let raw = scan(input)?;
    Ok(insert_implicit_mul(raw))
}

/// First pass: turn the character stream into tokens with no synthetic stars.
fn scan(input: &str) -> Result<Vec<Token>, EvalError> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            '0'..='9' | '.' => {
                let (num, next) = scan_number(&chars, i)?;
                out.push(Token::Number(num));
                i = next;
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            // Accept the pretty Unicode minus as well as ASCII, so a display
            // string can be re-tokenized if that is ever useful.
            '-' | '\u{2212}' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' | '\u{00D7}' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' | '\u{00F7}' => {
                out.push(Token::Slash);
                i += 1;
            }
            '^' => {
                out.push(Token::Caret);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '!' => {
                out.push(Token::Bang);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '\u{221A}' => {
                out.push(Token::Sqrt);
                i += 1;
            }
            '\u{03C0}' => {
                out.push(Token::Pi);
                i += 1;
            }
            'e' => {
                // A bare `e` is the constant; a function beginning with `e`
                // (`exp`) is handled by the alphabetic branch below. Because
                // `e` reaches here first, disambiguate by peeking.
                if let Some(f) = match_func(&chars, i) {
                    out.push(Token::Func(f.1));
                    i += f.0;
                } else {
                    out.push(Token::E);
                    i += 1;
                }
            }
            'a'..='z' => {
                if let Some(f) = match_func(&chars, i) {
                    out.push(Token::Func(f.1));
                    i += f.0;
                } else {
                    return Err(EvalError::Syntax);
                }
            }
            _ => return Err(EvalError::Syntax),
        }
    }

    Ok(out)
}

/// Try to match a function name at `chars[i]`. Returns `(consumed_len, name)`.
///
/// The trailing `(` is intentionally *not* consumed here: the caller lets the
/// normal scan emit it as an [`Token::LParen`] so the parser sees the same
/// `Func LParen` shape whether or not the user typed the paren explicitly.
fn match_func(chars: &[char], i: usize) -> Option<(usize, FuncName)> {
    for (name, f) in FUNCS {
        let len = name.chars().count();
        if i + len <= chars.len() && chars[i..i + len].iter().copied().eq(name.chars()) {
            return Some((len, *f));
        }
    }
    None
}

/// Scan a numeric literal starting at `i`. Rejects a second decimal point.
fn scan_number(chars: &[char], i: usize) -> Result<(f64, usize), EvalError> {
    let start = i;
    let mut j = i;
    let mut seen_dot = false;
    while j < chars.len() {
        match chars[j] {
            '0'..='9' => j += 1,
            '.' if !seen_dot => {
                seen_dot = true;
                j += 1;
            }
            _ => break,
        }
    }
    let s: String = chars[start..j].iter().collect();
    // A lone "." is not a number.
    if s == "." {
        return Err(EvalError::Syntax);
    }
    let value: f64 = s.parse().map_err(|_| EvalError::Syntax)?;
    Ok((value, j))
}

/// True if a token can *end* a value (so a following value-opener implies `*`).
fn ends_value(t: &Token) -> bool {
    matches!(
        t,
        Token::Number(_) | Token::RParen | Token::Pi | Token::E | Token::Bang | Token::Percent
    )
}

/// True if a token can *begin* a value (so a preceding value-ender implies `*`).
fn begins_value(t: &Token) -> bool {
    matches!(
        t,
        Token::Number(_)
            | Token::LParen
            | Token::Pi
            | Token::E
            | Token::Sqrt
            | Token::Func(_)
    )
}

/// Second pass: insert a synthetic [`Token::Star`] between any value-ender and a
/// following value-beginner. This is what makes `2π`, `2(3)`, `(1+2)(3)`,
/// `2√9` and `2sin(0)` mean multiplication.
fn insert_implicit_mul(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for t in tokens {
        if let Some(prev) = out.last() {
            // Two adjacent NUMBER literals are never juxtaposed multiplication —
            // they can only come from malformed input like `3.5.2`. Leaving no
            // synthetic star between them makes the parser reject the second
            // number as an unexpected token (a syntax error), which is correct.
            let both_numbers = matches!(prev, Token::Number(_)) && matches!(t, Token::Number(_));
            if !both_numbers && ends_value(prev) && begins_value(&t) {
                out.push(Token::Star);
            }
        }
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Token> {
        tokenize(s).unwrap()
    }

    #[test]
    fn plain_number() {
        assert_eq!(toks("42"), vec![Token::Number(42.0)]);
    }

    #[test]
    fn decimal_number() {
        assert_eq!(toks("3.5"), vec![Token::Number(3.5)]);
    }

    #[test]
    fn double_dot_tokenizes_as_two_numbers() {
        // `3.5.2` scans as two numbers with NO synthetic star between them, so
        // the parser (not the tokenizer) rejects it. Here we just confirm the
        // tokenizer emits the two adjacent numbers unglued.
        assert_eq!(
            toks("3.5.2"),
            vec![Token::Number(3.5), Token::Number(0.2)]
        );
    }

    #[test]
    fn lone_dot_is_error() {
        assert!(tokenize(".").is_err());
    }

    #[test]
    fn implicit_number_pi() {
        assert_eq!(
            toks("2\u{03C0}"),
            vec![Token::Number(2.0), Token::Star, Token::Pi]
        );
    }

    #[test]
    fn implicit_number_paren() {
        assert_eq!(
            toks("2(3)"),
            vec![
                Token::Number(2.0),
                Token::Star,
                Token::LParen,
                Token::Number(3.0),
                Token::RParen
            ]
        );
    }

    #[test]
    fn implicit_paren_paren() {
        // (1+2)(3)
        let t = toks("(1+2)(3)");
        assert!(t.contains(&Token::Star));
        // The star sits between the ) and the ( .
        let idx = t.iter().position(|x| *x == Token::RParen).unwrap();
        assert_eq!(t[idx + 1], Token::Star);
        assert_eq!(t[idx + 2], Token::LParen);
    }

    #[test]
    fn implicit_number_sqrt() {
        assert_eq!(
            toks("2\u{221A}9"),
            vec![
                Token::Number(2.0),
                Token::Star,
                Token::Sqrt,
                Token::Number(9.0)
            ]
        );
    }

    #[test]
    fn implicit_number_func() {
        let t = toks("2sin(0)");
        assert_eq!(t[0], Token::Number(2.0));
        assert_eq!(t[1], Token::Star);
        assert_eq!(t[2], Token::Func(FuncName::Sin));
        assert_eq!(t[3], Token::LParen);
    }

    #[test]
    fn e_constant_vs_exp_function() {
        assert_eq!(toks("e"), vec![Token::E]);
        let t = toks("exp(0)");
        assert_eq!(t[0], Token::Func(FuncName::Exp));
        assert_eq!(t[1], Token::LParen);
    }

    #[test]
    fn inverse_funcs_scan() {
        assert_eq!(toks("asin(")[0], Token::Func(FuncName::Asin));
        assert_eq!(toks("acos(")[0], Token::Func(FuncName::Acos));
        assert_eq!(toks("atan(")[0], Token::Func(FuncName::Atan));
    }

    #[test]
    fn no_spurious_star_after_operator() {
        // 2*3 must not gain a synthetic star.
        assert_eq!(
            toks("2*3"),
            vec![Token::Number(2.0), Token::Star, Token::Number(3.0)]
        );
    }

    #[test]
    fn const_times_paren() {
        let t = toks("\u{03C0}(2)");
        assert_eq!(t[0], Token::Pi);
        assert_eq!(t[1], Token::Star);
        assert_eq!(t[2], Token::LParen);
    }
}
