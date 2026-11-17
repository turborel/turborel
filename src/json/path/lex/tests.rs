//! Tests of the tokens of an SQL/JSON path expression: separators,
//! special symbols, identifiers, numeric and string literals.

use super::*;
use crate::error::SqlError;
use crate::value::{DecFloat, MAX_DECFLOAT_PRECISION, Value};

/// `text` with each `%` a reverse solidus and `u`, the start of an escape
/// of four hexadecimal digits.
fn escapes(text: &str) -> String {
    text.replace('%', "\\u")
}

/// Each token of `text`, described: a symbol by its name, an identifier
/// as `id:` and its name, a string as `str:` and its value, a number as
/// `num`.
fn tokens(text: &str) -> Vec<String> {
    lex(text).unwrap().into_iter().map(|lexeme| describe(&lexeme.token)).collect()
}

fn describe(token: &Token) -> String {
    match token {
        Token::Symbol(symbol) => format!("{symbol:?}"),
        Token::Identifier(name) => format!("id:{name}"),
        Token::String(string) => format!("str:{string}"),
        Token::Number(..) => "num".to_owned(),
    }
}

fn refused(text: &str) -> SqlError {
    lex(text).unwrap_err()
}

/// The value of the one numeric literal `text` is.
fn number(text: &str) -> Value {
    let lexemes = lex(text).unwrap();
    assert_eq!(lexemes.len(), 1, "{text}");
    let Some(Lexeme { token: Token::Number(Item::Scalar(scalar), _), .. }) = lexemes.first() else { panic!("a number: {text}") };
    scalar.value().clone()
}

/// The exact number `text` is, as its units and scale.
fn exact_of(text: &str) -> (i128, u32) {
    let Value::Exact(d) = number(text) else { panic!("an exact number: {text}") };
    (d.units, d.scale)
}

/// The bits of the approximate number `text` is.
fn bits(text: &str) -> u64 {
    let Value::Approx(x) = number(text) else { panic!("an approximate number: {text}") };
    x.to_bits()
}

/// The value of the one string literal `text` is.
fn string(text: &str) -> String {
    let tokens = tokens(text);
    assert_eq!(tokens.len(), 1, "{text}");
    tokens.first().unwrap().strip_prefix("str:").unwrap().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 SR2] [ECMAScript 5.1
/// §7.2, §7.3] white space, TAB, VT, FF, SP, NBSP, BOM and a Zs character,
/// and line terminators, LF, CR, LS and PS, separate tokens and are
/// insignificant around them; an empty text has no tokens.
#[test]
fn white_space_and_line_terminators_separate_tokens() {
    assert_eq!(tokens(""), Vec::<String>::new());
    assert_eq!(tokens(" \t\n"), Vec::<String>::new());
    let separated = "\t$\u{B}a\u{C}b c\u{A0}d\u{FEFF}e\u{2003}f\u{3000}g\nh\ri\u{2028}j\u{2029}k";
    assert_eq!(tokens(separated), ["id:$", "id:a", "id:b", "id:c", "id:d", "id:e", "id:f", "id:g", "id:h", "id:i", "id:j", "id:k"]);
    assert_eq!(tokens(" lax  $ "), ["id:lax", "id:$"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 F, SR2c, SR2e] each
/// `<SQL/JSON special symbol>` is a token, `@` among them, `<>` and `!=`
/// one token, and the longest is taken; a character no token begins with,
/// `=`, `&` or `|` alone among them, and a comment's start, is SQLCODE
/// -101.
#[test]
fn special_symbols_are_tokens() {
    let expected = [
        "NotEquals",
        "NotEquals",
        "LessEquals",
        "GreaterEquals",
        "DoubleEquals",
        "DoubleAmpersand",
        "DoubleBar",
        "Exclamation",
        "Less",
        "Greater",
        "At",
        "Asterisk",
        "Comma",
        "LeftBracket",
        "RightBracket",
        "LeftParen",
        "RightParen",
        "Minus",
        "Plus",
        "Percent",
        "Question",
        "Solidus",
        "Period",
    ];
    assert_eq!(tokens("!= <> <= >= == && || ! < > @ * , [ ] ( ) - + % ? / ."), expected);
    assert_eq!(tokens("<=>"), ["LessEquals", "Greater"]);
    assert_eq!(tokens("!!="), ["Exclamation", "NotEquals"]);
    assert_eq!(tokens("$.a"), ["id:$", "Period", "id:a"]);
    for text in ["=", "!==", "a = b", "&", "|", "a & b", "#", ";", "{", "~", "`"] {
        assert_eq!(refused(text).sqlcode(), -101_i32, "{text}");
    }
    assert_eq!(tokens("/* */"), ["Solidus", "Asterisk", "Asterisk", "Solidus"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 SR2d, SR4] [ECMAScript
/// 5.1 §7.6] an identifier begins with a UnicodeLetter (Lu, Ll, Lt, Lm,
/// Lo, Nl), `$` or `_`, and goes on with those, digits (Nd), combining
/// marks (Mn, Mc), connector punctuation (Pc), ZWNJ and ZWJ; key words
/// are identifiers; a digit or mark does not begin one.
#[test]
fn identifiers_are_unicode_names() {
    assert_eq!(
        tokens("$ $a _x a1 $1 a$b lax strict last"),
        ["id:$", "id:$a", "id:_x", "id:a1", "id:$1", "id:a$b", "id:lax", "id:strict", "id:last"]
    );
    assert_eq!(tokens("é ǅ ʰ 中 Ⅷ"), ["id:é", "id:ǅ", "id:ʰ", "id:中", "id:Ⅷ"]);
    assert_eq!(
        tokens("e\u{301} a٣ a\u{203F}b a\u{200C}b a\u{200D}b a\u{903}"),
        ["id:e\u{301}", "id:a٣", "id:a\u{203F}b", "id:a\u{200C}b", "id:a\u{200D}b", "id:a\u{903}"]
    );
    assert_eq!(tokens("a.b"), ["id:a", "Period", "id:b"]);
    for text in ["\u{301}a", "٣", "\u{203F}", "\u{200C}a", "\u{A7}", "a \u{2122}"] {
        assert_eq!(refused(text).sqlcode(), -101_i32, "{text:?}");
    }
}

/// Verifies: [EXTENSION] [ECMAScript 5.1 §7.6] a `\u` escape in an
/// identifier contributes the character it stands for, which must be one
/// the identifier may have there: a letter or `$` first, a digit after;
/// an escape of a surrogate, cut short or of another letter is SQLCODE
/// -101.
#[test]
fn identifiers_may_hold_unicode_escapes() {
    assert_eq!(
        tokens(&escapes("%0061bc a%0062 %0024 %0024x %006C%0061%0078 a%0031")),
        ["id:abc", "id:ab", "id:$", "id:$x", "id:lax", "id:a1"]
    );
    for text in ["%0031a", "a%0020", "%0301x", "%D800", "%DC00", "%006", "%00G1", "\\x41", "\\", "a\\"] {
        assert_eq!(refused(&escapes(text)).sqlcode(), -101_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 GR1a] [ECMAScript 5.1
/// §7.8.3] a DecimalLiteral without an ExponentPart is the exact number
/// of the `<signed numeric literal>` of its characters, digits before or
/// after its period optional; one of more digits than an exact number
/// holds is a decimal floating-point number.
#[test]
fn a_decimal_literal_without_an_exponent_is_exact() {
    assert_eq!(exact_of("0"), (0_i128, 0_u32));
    assert_eq!(exact_of("12"), (12_i128, 0_u32));
    assert_eq!(exact_of("1.50"), (150_i128, 2_u32));
    assert_eq!(exact_of("0.001"), (1_i128, 3_u32));
    assert_eq!(exact_of(".5"), (5_i128, 1_u32));
    assert_eq!(exact_of("5."), (5_i128, 0_u32));
    assert_eq!(exact_of("0."), (0_i128, 0_u32));
    let held = DecFloat::of_digits(false, "1234567890123456789", 0, MAX_DECFLOAT_PRECISION).unwrap();
    assert!(matches!(number("1234567890123456789"), Value::DecFloat(d) if d == held));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 GR1a] [ECMAScript 5.1
/// §7.8.3] a DecimalLiteral with an ExponentPart is the approximate number
/// of its characters, the exponent signed or not, E of either case, the
/// forms `.5e1` and `5.e1` too; one past an approximate number's range is
/// SQLCODE -900.
#[test]
fn a_decimal_literal_with_an_exponent_is_approximate() {
    assert_eq!(bits("1e2"), 100.0_f64.to_bits());
    assert_eq!(bits("1.5E+3"), 1500.0_f64.to_bits());
    assert_eq!(bits("25e-1"), 2.5_f64.to_bits());
    assert_eq!(bits(".5e1"), 5.0_f64.to_bits());
    assert_eq!(bits("5.e-1"), 0.5_f64.to_bits());
    assert_eq!(bits("0e0"), 0.0_f64.to_bits());
    assert_eq!(refused("1E400").sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] [ECMAScript 5.1 §7.8.3] what is no numeric
/// literal: a leading zero, as of an OctalIntegerLiteral, a literal
/// followed at once by a digit, an identifier's first character or a
/// `\`, an exponent without digits, `0x` without a hexadecimal digit or
/// followed by a letter that is none.
#[test]
fn what_is_no_numeric_literal() {
    for text in ["01", "00.5", "3in", "1$", "1_", "1é", "1\\", "1e", "1e+", "1E-", "1.e", "0x", "0xg", "0x1g", "0x1$", "1.5.5e"] {
        assert_eq!(refused(text).sqlcode(), -101_i32, "{text}");
    }
    assert_eq!(tokens("1.5.a 1 .a 1..a"), ["num", "Period", "id:a", "num", "Period", "id:a", "num", "Period", "id:a"]);
    assert_eq!(tokens("1+2 1\u{2003}"), ["num", "Plus", "num", "num"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 Conformance Rule 1,
/// GR1a] [ECMAScript 5.1 §7.8.3] Feature T840: a HexIntegerLiteral, `0x`
/// or `0X` and hexadecimal digits of either case, is the exact number of
/// its MV, of scale 0; of more than 18 decimal digits a DECFLOAT rounded to
/// 34; past every DECFLOAT, SQLCODE -900. It is no `<unsigned integer>`.
#[test]
fn hex_integer_literals_are_exact_numbers() {
    assert_eq!(exact_of("0x1F"), (31_i128, 0_u32));
    assert_eq!(exact_of("0XaB"), (171_i128, 0_u32));
    assert_eq!(exact_of("0x0"), (0_i128, 0_u32));
    assert_eq!(exact_of("0x000010"), (16_i128, 0_u32));
    assert_eq!(exact_of("0xDE0B6B3A763FFFF"), (999_999_999_999_999_999_i128, 0_u32));
    let Value::DecFloat(big) = number("0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF") else { panic!("a DECFLOAT") };
    let expected = DecFloat::of_digits(false, "340282366920938463463374607431768211455", 0, MAX_DECFLOAT_PRECISION).unwrap();
    assert_eq!(big.to_string(), expected.to_string());
    let huge = format!("0x{}", "F".repeat(6000));
    assert_eq!(refused(&huge).sqlcode(), -900_i32);
    assert_eq!(refused(&format!("0x1{}", "0".repeat(5200))).sqlcode(), -900_i32);
    assert_eq!(tokens("0x1F+0x2 0x3)"), ["num", "Plus", "num", "num", "RightParen"]);
    let numerals: Vec<Numeral> = lex("0x5 5 5. 5.0 5e0 .5")
        .unwrap()
        .into_iter()
        .map(|lexeme| if let Token::Number(_, numeral) = lexeme.token { numeral } else { panic!("a number") })
        .collect();
    assert_eq!(numerals, [Numeral::Other, Numeral::Integer, Numeral::Other, Numeral::Other, Numeral::Other, Numeral::Other]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 GR1b] [ECMAScript 5.1
/// §7.8.4] a string literal in double or single quotation marks is its
/// characters, the other mark among them, each escape replaced: the
/// single character escapes, `\0`, `\x` of two hexadecimal digits, `\u` of
/// four, two `\u` escapes of a surrogate pair one character, any other
/// escaped character itself, and a line continuation nothing.
#[test]
fn a_string_literal_is_its_characters_unescaped() {
    assert_eq!(string("\"\""), "");
    assert_eq!(string("''"), "");
    assert_eq!(string("\"a'b\""), "a'b");
    assert_eq!(string("'a\"b'"), "a\"b");
    assert_eq!(string(r#""\b\t\n\v\f\r\"\'\\""#), "\u{8}\t\n\u{B}\u{C}\r\"'\\");
    assert_eq!(string(r"'a\0b'"), "a\0b");
    assert_eq!(string(r"'\x41\x6a\xFF'"), "Aj\u{FF}");
    assert_eq!(string(&escapes("'%0041%00e9%00E9'")), "Aéé");
    assert_eq!(string(&escapes("'%D834%DD1E'")), "\u{1D11E}");
    assert_eq!(string(r"'\q\$\ \é'"), "q$ é");
    assert_eq!(string("'a\\\nb\\\rc\\\r\nd\\\u{2028}e\\\u{2029}f'"), "abcdef");
    assert_eq!(string("'é\u{1D11E}\u{A0}\t'"), "é\u{1D11E}\u{A0}\t");
}

/// Verifies: [EXTENSION] [ECMAScript 5.1 §7.8.4] what is no string
/// literal: one not closed, a line terminator in it not escaped, an
/// escape of a digit other than `\0` alone (an OctalEscapeSequence), a
/// `\x` or `\u` escape cut short; \[DESIGN\] and an escape of a surrogate
/// not of a pair, no Unicode character.
#[test]
fn what_is_no_string_literal() {
    let texts = [
        "'a",
        "'a\\",
        "\"a'",
        "'a\nb'",
        "'a\rb'",
        "'a\u{2028}b'",
        "'a\u{2029}b'",
        "'\\1'",
        "'\\01'",
        "'\\x4'",
        "'\\x4g'",
        "'%004'",
        "'%D834'",
        "'%D834x'",
        "'%D834\\n'",
        "'%D834%0041'",
        "'%D834%D834'",
        "'%DD1E'",
        "'%D834%DD1'",
    ];
    for text in texts {
        assert_eq!(refused(&escapes(text)).sqlcode(), -101_i32, "{text:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.45 SR2] a token is where
/// its first character is, counted from 0; a refusal names its rule and
/// the character it was refused at, counted from 1.
#[test]
fn tokens_and_refusals_know_their_place() {
    let places: Vec<usize> = lex("lax  $.a == 'x'").unwrap().iter().map(|lexeme| lexeme.at).collect();
    assert_eq!(places, [0_usize, 5, 6, 7, 9, 12]);
    let error = refused("lax $ = 1");
    assert_eq!(error.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.45 SR2"));
    assert!(error.msg.contains("(at character 7)"), "{}", error.msg);
    assert_eq!(error.sqlstate().as_str(), "2A000");
    let open = refused("$ ? ('abc");
    assert!(open.msg.contains("(at character 6)"), "{}", open.msg);
}
