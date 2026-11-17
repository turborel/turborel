//! The declarations of a prolog [XQuery Appendix A.1 \[7\] to \[28\]]:
//! each begun by `declare` or `import` and the keyword after it, as none
//! is reserved [XQuery Appendix A.2].

use crate::error::Result;
use crate::xquery::ast::{
    BoundarySpace, Construction, Declaration, DefaultNamespaceDecl, FunctionDecl, Import, InheritMode, ModuleImport, NamespaceDecl,
    NamespaceKind, OptionDecl, OrderingMode, Param, PreserveMode, SchemaImport, SchemaPrefix, Setter, Setup, VarDecl,
};
use crate::xquery::error::{ErrorCode, xquery_error};
use crate::xquery::keywords::{
    AT, BASE_URI, BOUNDARY_SPACE, COLLATION, CONSTRUCTION, COPY_NAMESPACES, DECLARE, DEFAULT, ELEMENT, EMPTY, EXTERNAL, FUNCTION, IMPORT,
    INHERIT, MODULE, NAMESPACE, NO_INHERIT, NO_PRESERVE, OPTION, ORDER, ORDERED, ORDERING, PRESERVE, SCHEMA, STRIP, UNORDERED, VARIABLE,
};
use crate::xquery::lexer::{Sym, Tok};
use crate::xquery::parser::Parser;

/// The prefix `import` binds, and the namespace it binds it to, if it
/// binds one [XQuery Appendix A.1 \[22\], \[23\]].
pub(super) fn import_prefix(import: &Import) -> Option<(&str, &str)> {
    match import {
        Import::Schema(SchemaImport { prefix: Some(SchemaPrefix::Namespace(prefix)), uri, .. })
        | Import::Module(ModuleImport { prefix: Some(prefix), uri, .. }) => Some((prefix, uri)),
        Import::Schema(_) | Import::Module(_) => None,
    }
}

impl Parser {
    /// The unprefixed name `n` tokens after the next, if it is one.
    fn keyword_at(&mut self, n: usize) -> Option<String> {
        match self.peek(n)? {
            Tok::Name(name) => name.prefix.is_none().then(|| name.local.clone()),
            Tok::PrefixWildcard(_)
            | Tok::LocalWildcard(_)
            | Tok::Integer(_)
            | Tok::Decimal(_)
            | Tok::Double(_)
            | Tok::String(_)
            | Tok::Pragma(_)
            | Tok::Sym(_)
            | Tok::End => None,
        }
    }

    /// A declaration of a prolog's first part [XQuery Appendix A.1 \[6\]],
    /// if one is next: a `DefaultNamespaceDecl` \[12\], `Setter` \[7\],
    /// `NamespaceDecl` \[10\] or `Import` \[8\].
    pub(super) fn setup(&mut self) -> Result<Option<Setup>> {
        let first = self.keyword_at(0);
        let second = self.keyword_at(1).unwrap_or_default();
        if first.as_deref() == Some(IMPORT) {
            return match second.as_str() {
                SCHEMA => self.schema_import().map(|i| Some(Setup::Import(Import::Schema(i)))),
                MODULE => self.module_import().map(|i| Some(Setup::Import(Import::Module(i)))),
                _ => Ok(None),
            };
        }
        if first.as_deref() != Some(DECLARE) {
            return Ok(None);
        }
        let third = self.keyword_at(2).unwrap_or_default();
        let rule = "a prolog's declaration is as its production writes it [XQuery Appendix A.1 [7], [10], [12]]";
        let setup = match (second.as_str(), third.as_str()) {
            (DEFAULT, ELEMENT) | (DEFAULT, FUNCTION) => Setup::DefaultNamespace(self.default_namespace_decl(rule)?),
            (DEFAULT, COLLATION) => Setup::Setter(Setter::DefaultCollation(self.after_keywords(3, rule)?)),
            (DEFAULT, ORDER) => {
                self.skip(3);
                self.expect_keyword(
                    EMPTY,
                    "an empty order declaration is 'declare default order empty', then 'greatest' or 'least' [XQuery Appendix A.1 [15]]",
                )?;
                Setup::Setter(Setter::EmptyOrder(self.empty_order()?))
            }
            (BOUNDARY_SPACE, _) => {
                Setup::Setter(Setter::BoundarySpace(self.choice(BoundarySpace::Preserve, BoundarySpace::Strip, rule)?))
            }
            (BASE_URI, _) => Setup::Setter(Setter::BaseUri(self.after_keywords(2, rule)?)),
            (CONSTRUCTION, _) => Setup::Setter(Setter::Construction(self.choice(Construction::Preserve, Construction::Strip, rule)?)),
            (ORDERING, _) => Setup::Setter(Setter::OrderingMode(self.ordering_mode(rule)?)),
            (COPY_NAMESPACES, _) => self.copy_namespaces_decl()?,
            (NAMESPACE, _) => Setup::Namespace(self.namespace_decl(rule)?),
            _ => return Ok(None),
        };
        Ok(Some(setup))
    }

    /// Takes `count` tokens.
    fn skip(&mut self, count: usize) {
        for _ in 0..count {
            self.ahead.pop_front();
        }
    }

    /// The `URILiteral` after the `count` keywords next.
    fn after_keywords(&mut self, count: usize, rule: &str) -> Result<String> {
        self.skip(count);
        self.expect_string(rule)
    }

    /// `preserve` as `preserve`, or `strip` as `strip`, after the two
    /// keywords next [XQuery Appendix A.1 \[11\], \[25\]].
    fn choice<T>(&mut self, preserve: T, strip: T, rule: &str) -> Result<T> {
        self.skip(2);
        if self.accept_keyword(PRESERVE) {
            Ok(preserve)
        } else if self.accept_keyword(STRIP) {
            Ok(strip)
        } else {
            Err(self.unexpected(rule))
        }
    }

    /// `OrderingModeDecl`'s mode [XQuery Appendix A.1 \[14\]].
    fn ordering_mode(&mut self, rule: &str) -> Result<OrderingMode> {
        self.skip(2);
        if self.accept_keyword(ORDERED) {
            Ok(OrderingMode::Ordered)
        } else if self.accept_keyword(UNORDERED) {
            Ok(OrderingMode::Unordered)
        } else {
            Err(self.unexpected(rule))
        }
    }

    /// `DefaultNamespaceDecl` [XQuery Appendix A.1 \[12\]].
    fn default_namespace_decl(&mut self, rule: &str) -> Result<DefaultNamespaceDecl> {
        self.skip(2);
        let kind = if self.accept_keyword(ELEMENT) { NamespaceKind::Element } else { NamespaceKind::Function };
        if kind == NamespaceKind::Function {
            self.ahead.pop_front();
        }
        self.expect_keyword(NAMESPACE, rule)?;
        Ok(DefaultNamespaceDecl { kind, uri: self.expect_string(rule)? })
    }

    /// `CopyNamespacesDecl` [XQuery Appendix A.1 \[16\]] to \[18\].
    fn copy_namespaces_decl(&mut self) -> Result<Setup> {
        let rule = "a copy-namespaces declaration is 'declare copy-namespaces', 'preserve' or 'no-preserve', ',', 'inherit' or 'no-inherit' [XQuery Appendix A.1 [16]]";
        self.skip(2);
        let preserve = if self.accept_keyword(PRESERVE) {
            PreserveMode::Preserve
        } else {
            self.expect_keyword(NO_PRESERVE, rule)?;
            PreserveMode::NoPreserve
        };
        self.expect_sym(Sym::Comma, rule)?;
        let inherit = if self.accept_keyword(INHERIT) {
            InheritMode::Inherit
        } else {
            self.expect_keyword(NO_INHERIT, rule)?;
            InheritMode::NoInherit
        };
        Ok(Setup::Setter(Setter::CopyNamespaces(preserve, inherit)))
    }

    /// `NamespaceDecl` [XQuery Appendix A.1 \[10\]].
    fn namespace_decl(&mut self, rule: &str) -> Result<NamespaceDecl> {
        self.skip(2);
        let prefix = self.expect_ncname(rule)?;
        self.expect_sym(Sym::Eq, rule)?;
        Ok(NamespaceDecl { prefix, uri: self.expect_string(rule)? })
    }

    /// `SchemaImport` [XQuery Appendix A.1 \[21\]] and its `SchemaPrefix`
    /// \[22\]: a prefix bound only to a target namespace \[err:XQST0057\]
    /// [XQuery §4.10].
    fn schema_import(&mut self) -> Result<SchemaImport> {
        let rule = "a schema import is 'import schema', a prefix if bound, a URI literal, then location hints after 'at' [XQuery Appendix A.1 [21], [22]]";
        self.skip(2);
        let at = self.here();
        let prefix = if self.accept_keyword(NAMESPACE) {
            let prefix = self.expect_ncname(rule)?;
            self.expect_sym(Sym::Eq, rule)?;
            Some(SchemaPrefix::Namespace(prefix))
        } else if self.accept_keyword(DEFAULT) {
            self.expect_keyword(ELEMENT, rule)?;
            self.expect_keyword(NAMESPACE, rule)?;
            Some(SchemaPrefix::DefaultElement)
        } else {
            None
        };
        let uri = self.expect_string(rule)?;
        if uri.is_empty() && matches!(prefix, Some(SchemaPrefix::Namespace(_))) {
            let why = "a schema import that binds a prefix has a target namespace [XQuery §4.10]";
            return Err(xquery_error(ErrorCode::XQST0057, self.lexer.place(at), why));
        }
        Ok(SchemaImport { prefix, uri, at: self.location_hints(rule)? })
    }

    /// `ModuleImport` [XQuery Appendix A.1 \[23\]]: its target namespace of
    /// nonzero length \[err:XQST0088\] [XQuery §4.11].
    fn module_import(&mut self) -> Result<ModuleImport> {
        let rule = "a module import is 'import module', 'namespace', a prefix and '=' if bound, a URI literal, then location hints after 'at' [XQuery Appendix A.1 [23]]";
        self.skip(2);
        let prefix = if self.accept_keyword(NAMESPACE) {
            let prefix = self.expect_ncname(rule)?;
            self.expect_sym(Sym::Eq, rule)?;
            Some(prefix)
        } else {
            None
        };
        let at = self.here();
        let uri = self.expect_string(rule)?;
        if uri.is_empty() {
            let why = "a module import's target namespace is of nonzero length [XQuery §4.11]";
            return Err(xquery_error(ErrorCode::XQST0088, self.lexer.place(at), why));
        }
        Ok(ModuleImport { prefix, uri, at: self.location_hints(rule)? })
    }

    /// The location hints of an import, after `at` and separated by
    /// commas, if it has any [XQuery Appendix A.1 \[21\], \[23\]].
    fn location_hints(&mut self, rule: &str) -> Result<Vec<String>> {
        let mut hints = Vec::new();
        if self.accept_keyword(AT) {
            loop {
                hints.push(self.expect_string(rule)?);
                if !self.accept_sym(Sym::Comma) {
                    break;
                }
            }
        }
        Ok(hints)
    }

    /// A declaration of a prolog's second part [XQuery Appendix A.1 \[6\]],
    /// if one is next: a `VarDecl` \[24\], `FunctionDecl` \[26\] or
    /// `OptionDecl` \[13\].
    pub(super) fn declaration(&mut self) -> Result<Option<Declaration>> {
        if !self.is_keyword(0, DECLARE) {
            return Ok(None);
        }
        let declaration = match self.keyword_at(1).unwrap_or_default().as_str() {
            VARIABLE => Declaration::Variable(self.var_decl()?),
            FUNCTION => Declaration::Function(self.function_decl()?),
            OPTION => {
                let rule = "an option declaration is 'declare option', a QName, then a string literal [XQuery Appendix A.1 [13]]";
                self.skip(2);
                let name = self.expect_name(rule)?;
                Declaration::Option(OptionDecl { name, value: self.expect_string(rule)? })
            }
            _ => return Ok(None),
        };
        Ok(Some(declaration))
    }

    /// `VarDecl` [XQuery Appendix A.1 \[24\]].
    fn var_decl(&mut self) -> Result<VarDecl> {
        let rule = "a variable declaration is 'declare variable', '$' and a QName, a type if declared, then ':=' and an expression or 'external' [XQuery Appendix A.1 [24]]";
        self.skip(2);
        let name = self.var_name(rule)?;
        let declared = self.type_declaration()?;
        let value = if self.accept_keyword(EXTERNAL) {
            None
        } else {
            self.expect_sym(Sym::Assign, rule)?;
            Some(self.expr_single()?)
        };
        Ok(VarDecl { name, declared, value })
    }

    /// `FunctionDecl` [XQuery Appendix A.1 \[26\]], its parameters each
    /// named once \[err:XQST0039\] [XQuery §4.15].
    fn function_decl(&mut self) -> Result<FunctionDecl> {
        let rule = "a function declaration is 'declare function', a QName, its parameters in parentheses, 'as' and a type if declared, then its body in braces or 'external' [XQuery Appendix A.1 [26]]";
        self.skip(2);
        let name = self.expect_name(rule)?;
        self.expect_sym(Sym::LParen, rule)?;
        let mut params: Vec<Param> = Vec::new();
        if !self.accept_sym(Sym::RParen) {
            loop {
                let at = self.here();
                let param = Param { name: self.var_name(rule)?, declared: self.type_declaration()? };
                if params.iter().any(|p| p.name == param.name) {
                    let why = format!("a function declaration has two parameters named '{}' [XQuery §4.15]", param.name);
                    return Err(xquery_error(ErrorCode::XQST0039, self.lexer.place(at), &why));
                }
                params.push(param);
                if !self.accept_sym(Sym::Comma) {
                    break;
                }
            }
            self.expect_sym(Sym::RParen, rule)?;
        }
        let returns = self.type_declaration()?;
        let body = if self.accept_keyword(EXTERNAL) { None } else { Some(self.enclosed_expr(rule)?) };
        Ok(FunctionDecl { name, params, returns, body })
    }
}
