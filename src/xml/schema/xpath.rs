//! The restricted XPath expressions of identity-constraint definitions
//! [Schema1 §3.11.6]: a selector, paths of child steps from the context
//! element or, after `.//`, from any of its descendants; a field, the same
//! with a last step that may name an attribute. The child and attribute
//! axes may be written in full (`child::`, `attribute::`), and white
//! space may separate the tokens [XPath §3.7].

use super::builtin::resolve;
use super::component::Expanded;
use crate::xml::{Version, is_ncname};

/// A NameTest [Schema1 §3.11.6 \[4\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameTest {
    /// `*`: any name.
    Any,
    /// `prefix:*`: any name of the prefix's namespace.
    Namespace(Box<str>),
    /// A QName: an unprefixed name is of no namespace [XPath §2.3].
    Name(Expanded),
}

impl NameTest {
    /// Whether a node named `name` passes it.
    #[must_use]
    pub fn matches(&self, uri: Option<&str>, local: &str) -> bool {
        match self {
            NameTest::Any => true,
            NameTest::Namespace(ns) => uri == Some(&**ns),
            NameTest::Name(name) => name.uri.as_deref() == uri && &*name.local == local,
        }
    }
}

/// A Step [Schema1 §3.11.6 \[3\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `.`: the node itself.
    Itself,
    /// A child element passing the name test.
    Child(NameTest),
}

/// A Path [Schema1 §3.11.6 \[2\], \[5\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// Whether it begins `.//`, from the context node and its descendants.
    pub descendants: bool,
    /// Its steps.
    pub steps: Vec<Step>,
    /// Of a field, the attribute its last step names.
    pub attribute: Option<NameTest>,
}

/// A token of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token<'a> {
    /// `.`.
    Dot,
    /// `/`.
    Slash,
    /// `//`.
    DoubleSlash,
    /// `|`.
    Bar,
    /// `@`.
    At,
    /// `*`.
    Star,
    /// `:`.
    Colon,
    /// `::`.
    Axis,
    /// An NCName.
    Name(&'a str),
}

/// The tokens of `text`, white space between them dropped; none if a
/// character begins no token.
fn tokens(text: &str) -> Option<Vec<Token<'_>>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while let Some(c) = rest.chars().next() {
        let (token, len) = if rest.starts_with("//") {
            (Token::DoubleSlash, 2)
        } else if rest.starts_with("::") {
            (Token::Axis, 2)
        } else {
            match c {
                '.' => (Token::Dot, 1),
                '|' => (Token::Bar, 1),
                '@' => (Token::At, 1),
                '*' => (Token::Star, 1),
                '/' => (Token::Slash, 1),
                ':' => (Token::Colon, 1),
                _ => {
                    let end = rest.find(|ch: char| ch.is_whitespace() || "./|@*:".contains(ch)).unwrap_or(rest.len());
                    let name = rest.get(..end).filter(|n| is_ncname(Version::V11, n))?;
                    (Token::Name(name), end)
                }
            }
        };
        out.push(token);
        rest = rest.get(len..)?.trim_start();
    }
    Some(out)
}

/// The paths of `text`, a selector, or a field if `field` [Schema1
/// §3.11.6 c-selector-xpath, c-fields-xpaths], its prefixes resolved by
/// the namespaces in scope `scope`; none if it is no such expression.
#[must_use]
pub fn parse(text: &str, field: bool, scope: &[(Option<&str>, &str)]) -> Option<Vec<Path>> {
    let tokens = tokens(text)?;
    let mut paths = Vec::new();
    for part in tokens.split(|t| *t == Token::Bar) {
        paths.push(path(part, field, scope)?);
    }
    Some(paths)
}

/// The Path of `tokens`.
fn path(tokens: &[Token<'_>], field: bool, scope: &[(Option<&str>, &str)]) -> Option<Path> {
    let (descendants, rest) = match tokens {
        [Token::Dot, Token::DoubleSlash, rest @ ..] => (true, rest),
        rest => (false, rest),
    };
    let parts: Vec<&[Token<'_>]> = rest.split(|t| *t == Token::Slash).collect();
    let (last, steps) = parts.split_last()?;
    let mut out = Path { descendants, steps: Vec::new(), attribute: None };
    for part in steps {
        out.steps.push(step(part, scope)?);
    }
    match attribute_step(last).filter(|_| field) {
        Some(test) => out.attribute = Some(name_test(test, scope)?),
        None => out.steps.push(step(last, scope)?),
    }
    Some(out)
}

/// The name test of `tokens` if they are a step of the attribute axis,
/// `@` or `attribute::` and a name test.
fn attribute_step<'t, 'a>(tokens: &'t [Token<'a>]) -> Option<&'t [Token<'a>]> {
    match tokens {
        [Token::At, test @ ..] | [Token::Name("attribute"), Token::Axis, test @ ..] => Some(test),
        _ => None,
    }
}

/// The Step of `tokens`: `.`, or a name test, written `child::` or not.
fn step(tokens: &[Token<'_>], scope: &[(Option<&str>, &str)]) -> Option<Step> {
    if tokens == [Token::Dot] {
        return Some(Step::Itself);
    }
    let test = tokens.strip_prefix(&[Token::Name("child"), Token::Axis][..]).unwrap_or(tokens);
    Some(Step::Child(name_test(test, scope)?))
}

/// The NameTest of `tokens` [Schema1 §3.11.6 \[4\]]: `*`, `NCName:*` or
/// a QName, its prefix resolved by `scope`; an unprefixed name of no
/// namespace [XPath §2.3].
fn name_test(tokens: &[Token<'_>], scope: &[(Option<&str>, &str)]) -> Option<NameTest> {
    match tokens {
        [Token::Star] => Some(NameTest::Any),
        [Token::Name(prefix), Token::Colon, Token::Star] => {
            let uri = scope.iter().find(|(p, _)| *p == Some(*prefix)).map(|(_, uri)| *uri)?;
            Some(NameTest::Namespace(uri.into()))
        }
        [Token::Name(local)] => Some(NameTest::Name(Expanded::new(None, local))),
        [Token::Name(prefix), Token::Colon, Token::Name(local)] => resolve(&format!("{prefix}:{local}"), scope).map(NameTest::Name),
        _ => None,
    }
}
