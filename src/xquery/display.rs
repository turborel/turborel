//! A tree written back as XQuery text [XQuery Appendix A.1], which parses
//! to the same tree: keywords and operators between spaces, string
//! literals in quotation marks, and each character a parse would change,
//! or read as markup, written as a reference. Comments are not kept, and
//! whitespace only where the grammar keeps it, in direct constructors.
//!
//! Modules and prologs are written here, expressions in
//! `display/expr.rs`, constructors in `display/direct.rs`, and sequence
//! types and node tests in `display/types.rs`.

mod direct;
mod expr;
mod types;

#[cfg(test)]
mod tests;

use std::fmt::{self, Display, Formatter};

use super::ast::{
    BoundarySpace, Construction, Declaration, DefaultNamespaceDecl, EmptyOrder, Import, InheritMode, Module, ModuleBody, NamespaceKind,
    OrderingMode, PreserveMode, Prolog, QName, SchemaPrefix, SequenceType, Setter, Setup, VersionDecl,
};
use super::keywords::{
    AS, AT, BASE_URI, BOUNDARY_SPACE, COLLATION, CONSTRUCTION, COPY_NAMESPACES, DECLARE, DEFAULT, ELEMENT, EMPTY, ENCODING, EXTERNAL,
    FUNCTION, GREATEST, IMPORT, INHERIT, LEAST, MODULE, NAMESPACE, NO_INHERIT, NO_PRESERVE, OPTION, ORDER, ORDERED, ORDERING, PRESERVE,
    SCHEMA, STRIP, UNORDERED, VARIABLE, VERSION, XQUERY,
};

/// [XQuery Appendix A.2.3] The characters a parse changes to a line feed,
/// which a literal writes as character references: a carriage return,
/// and of XML 1.1 a next line and a line separator.
const LINE_ENDS: [char; 3] = ['\r', '\u{85}', '\u{2028}'];
/// [XQuery Appendix A.2.1 \[145\]] The predefined entities, by the
/// character each stands for.
const PREDEFINED: [(char, &str); 5] = [('<', "lt"), ('>', "gt"), ('&', "amp"), ('"', "quot"), ('\'', "apos")];

impl Display for QName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.prefix {
            Some(prefix) => write!(f, "{prefix}:{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}

/// Writes `value` as a string literal [XQuery Appendix A.2.1 \[144\]], in
/// quotation marks: each `"` doubled, each `&` and line end a reference.
pub(super) fn string_literal(f: &mut Formatter<'_>, value: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in value.chars() {
        if c == '"' {
            f.write_str("\"\"")?;
        } else {
            reference_or_char(f, c, &['&'])?;
        }
    }
    f.write_str("\"")
}

/// Writes `c` as it is, or as a reference if it is one of `markup` or a
/// line end a parse would change [XQuery Appendix A.2.3]: a predefined
/// entity reference if it has one, else a character reference.
pub(super) fn reference_or_char(f: &mut Formatter<'_>, c: char, markup: &[char]) -> fmt::Result {
    if markup.contains(&c) || LINE_ENDS.contains(&c) { reference(f, c) } else { write!(f, "{c}") }
}

/// Writes `c` as a reference [XQuery Appendix A.2.1 \[145\], \[153\]]:
/// a predefined entity reference if it has one, else a character
/// reference.
pub(super) fn reference(f: &mut Formatter<'_>, c: char) -> fmt::Result {
    match PREDEFINED.iter().find(|&&(p, _)| p == c) {
        Some((_, entity)) => write!(f, "&{entity};"),
        None => write!(f, "&#x{:X};", u32::from(c)),
    }
}

/// Writes `items` with `separator` between them.
pub(super) fn separated<T: Display>(f: &mut Formatter<'_>, items: &[T], separator: &str) -> fmt::Result {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            f.write_str(separator)?;
        }
        item.fmt(f)?;
    }
    Ok(())
}

/// Writes `word`, a keyword or an operator, between spaces.
pub(super) fn spaced(f: &mut Formatter<'_>, word: &str) -> fmt::Result {
    f.write_str(" ")?;
    f.write_str(word)?;
    f.write_str(" ")
}

/// Writes ` as T` if `declared` is a type [XQuery Appendix A.1 \[118\]].
pub(super) fn type_declaration(f: &mut Formatter<'_>, declared: Option<&SequenceType>) -> fmt::Result {
    declared.map_or(Ok(()), |t| write!(f, " {AS} {t}"))
}

impl Display for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(version) = &self.version {
            write!(f, "{version}")?;
        }
        match &self.body {
            ModuleBody::Main(prolog, body) => write!(f, "{prolog}{body}"),
            ModuleBody::Library(decl, prolog) => {
                write!(f, "{MODULE} {NAMESPACE} {} = ", decl.prefix)?;
                string_literal(f, &decl.uri)?;
                write!(f, "; {prolog}")
            }
        }
    }
}

impl Display for VersionDecl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{XQUERY} {VERSION} ")?;
        string_literal(f, &self.version)?;
        if let Some(encoding) = &self.encoding {
            write!(f, " {ENCODING} ")?;
            string_literal(f, encoding)?;
        }
        f.write_str("; ")
    }
}

impl Display for Prolog {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for setup in &self.setup {
            write!(f, "{setup}; ")?;
        }
        for declaration in &self.declarations {
            write!(f, "{declaration}; ")?;
        }
        Ok(())
    }
}

impl Display for Setup {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Setup::DefaultNamespace(decl) => write!(f, "{decl}"),
            Setup::Setter(setter) => write!(f, "{setter}"),
            Setup::Namespace(decl) => {
                write!(f, "{DECLARE} {NAMESPACE} {} = ", decl.prefix)?;
                string_literal(f, &decl.uri)
            }
            Setup::Import(import) => write!(f, "{import}"),
        }
    }
}

impl Display for DefaultNamespaceDecl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            NamespaceKind::Element => ELEMENT,
            NamespaceKind::Function => FUNCTION,
        };
        write!(f, "{DECLARE} {DEFAULT} {kind} {NAMESPACE} ")?;
        string_literal(f, &self.uri)
    }
}

impl Display for Setter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{DECLARE} ")?;
        match self {
            Setter::BoundarySpace(space) => {
                write!(f, "{BOUNDARY_SPACE} {}", if *space == BoundarySpace::Preserve { PRESERVE } else { STRIP })
            }
            Setter::DefaultCollation(uri) => {
                write!(f, "{DEFAULT} {COLLATION} ")?;
                string_literal(f, uri)
            }
            Setter::BaseUri(uri) => {
                write!(f, "{BASE_URI} ")?;
                string_literal(f, uri)
            }
            Setter::Construction(mode) => write!(f, "{CONSTRUCTION} {}", if *mode == Construction::Preserve { PRESERVE } else { STRIP }),
            Setter::OrderingMode(mode) => write!(f, "{ORDERING} {}", if *mode == OrderingMode::Ordered { ORDERED } else { UNORDERED }),
            Setter::EmptyOrder(order) => write!(f, "{DEFAULT} {ORDER} {EMPTY} {}", empty_order(*order)),
            Setter::CopyNamespaces(preserve, inherit) => {
                let preserve = if *preserve == PreserveMode::Preserve { PRESERVE } else { NO_PRESERVE };
                let inherit = if *inherit == InheritMode::Inherit { INHERIT } else { NO_INHERIT };
                write!(f, "{COPY_NAMESPACES} {preserve}, {inherit}")
            }
        }
    }
}

/// The keyword after `empty` [XQuery Appendix A.1 \[15\], \[41\]].
pub(super) fn empty_order(order: EmptyOrder) -> &'static str {
    match order {
        EmptyOrder::Greatest => GREATEST,
        EmptyOrder::Least => LEAST,
    }
}

impl Display for Import {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (uri, hints) = match self {
            Import::Schema(import) => {
                write!(f, "{IMPORT} {SCHEMA} ")?;
                match &import.prefix {
                    Some(SchemaPrefix::Namespace(prefix)) => write!(f, "{NAMESPACE} {prefix} = ")?,
                    Some(SchemaPrefix::DefaultElement) => write!(f, "{DEFAULT} {ELEMENT} {NAMESPACE} ")?,
                    None => {}
                }
                (&import.uri, &import.at)
            }
            Import::Module(import) => {
                write!(f, "{IMPORT} {MODULE} ")?;
                if let Some(prefix) = &import.prefix {
                    write!(f, "{NAMESPACE} {prefix} = ")?;
                }
                (&import.uri, &import.at)
            }
        };
        string_literal(f, uri)?;
        for (index, hint) in hints.iter().enumerate() {
            if index == 0 {
                write!(f, " {AT} ")?;
            } else {
                f.write_str(", ")?;
            }
            string_literal(f, hint)?;
        }
        Ok(())
    }
}

impl Display for Declaration {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Declaration::Variable(decl) => {
                write!(f, "{DECLARE} {VARIABLE} ${}", decl.name)?;
                type_declaration(f, decl.declared.as_ref())?;
                match &decl.value {
                    Some(value) => write!(f, " := {value}"),
                    None => write!(f, " {EXTERNAL}"),
                }
            }
            Declaration::Function(decl) => {
                write!(f, "{DECLARE} {FUNCTION} {}(", decl.name)?;
                for (index, param) in decl.params.iter().enumerate() {
                    write!(f, "{}${}", if index == 0 { "" } else { ", " }, param.name)?;
                    type_declaration(f, param.declared.as_ref())?;
                }
                f.write_str(")")?;
                type_declaration(f, decl.returns.as_ref())?;
                match &decl.body {
                    Some(body) => write!(f, " {{ {body} }}"),
                    None => write!(f, " {EXTERNAL}"),
                }
            }
            Declaration::Option(decl) => {
                write!(f, "{DECLARE} {OPTION} {} ", decl.name)?;
                string_literal(f, &decl.value)
            }
        }
    }
}
