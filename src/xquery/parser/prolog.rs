//! Modules and their prologs [XQuery §4] [XQuery Appendix A.1 \[1\] to
//! \[28\]], and the static errors a prolog's declarations show by
//! themselves: a setter or default namespace declared twice, a prefix
//! declared twice or as `xml` [XQuery §4.3 to §4.13], a version or
//! encoding not supported [XQuery §4.1], a function's parameter named
//! twice [XQuery §4.15]. Imports are analysed with the static context
//! (`xquery/analyze/import.rs`, `xquery/analyze/modules.rs`).

mod declarations;

use super::Parser;
use crate::error::Result;
use crate::xml::{XML_NAMESPACE, XML_PREFIX};
use crate::xquery::ast::{Module, ModuleBody, ModuleDecl, NamespaceKind, Prolog, Setter, Setup, VersionDecl};
use crate::xquery::error::{ErrorCode, xquery_error};
use crate::xquery::keywords::{ENCODING, MODULE, NAMESPACE, VERSION, XQUERY};
use crate::xquery::lexer::Sym;

/// [XQuery §4.1] The version of XQuery turborel reads.
const XQUERY_VERSION: &str = "1.0";
/// [XQuery §4.3 to §4.13] The prefix `xmlns`, which no namespace
/// declaration declares \[err:XQST0070\].
const XMLNS_PREFIX: &str = "xmlns";

/// What a prolog has declared so far, to find what it declares twice.
#[derive(Debug, Default)]
struct Declared {
    /// The setters and default namespaces declared, by what each sets.
    once: Vec<&'static str>,
    /// The namespace prefixes declared.
    prefixes: Vec<String>,
}

impl Parser {
    /// `Module` [XQuery Appendix A.1 \[1\]].
    pub(super) fn module(&mut self) -> Result<Module> {
        let version = if self.is_keyword(0, XQUERY) && self.is_keyword(1, VERSION) { Some(self.version_decl()?) } else { None };
        let mut declared = Declared::default();
        if self.is_keyword(0, MODULE) && self.is_keyword(1, NAMESPACE) {
            let decl = self.module_decl(&mut declared)?;
            let prolog = self.prolog(declared)?;
            return Ok(Module { version, body: ModuleBody::Library(decl, prolog) });
        }
        let prolog = self.prolog(declared)?;
        let body = self.expr()?;
        Ok(Module { version, body: ModuleBody::Main(prolog, body) })
    }

    /// `VersionDecl` [XQuery Appendix A.1 \[2\]], which `xquery version`
    /// begins: version 1.0 \[err:XQST0031\], and an encoding that is an
    /// `EncName` [XML 1.0 §4.3.3 \[81\]] \[err:XQST0087\] [XQuery §4.1].
    fn version_decl(&mut self) -> Result<VersionDecl> {
        let rule = "a version declaration is 'xquery version', a string literal, 'encoding' and a string literal if declared, then ';' [XQuery Appendix A.1 [2]]";
        self.ahead.pop_front();
        self.ahead.pop_front();
        let at = self.here();
        let version = self.expect_string(rule)?;
        if version != XQUERY_VERSION {
            let why = format!("the version declared, '{version}', is not 1.0, the version turborel reads [XQuery §4.1]");
            return Err(xquery_error(ErrorCode::XQST0031, self.lexer.place(at), &why));
        }
        let encoding = if self.accept_keyword(ENCODING) {
            let encoding_at = self.here();
            let encoding = self.expect_string(rule)?;
            if !is_enc_name(&encoding) {
                let why = format!("the encoding declared, '{encoding}', is no EncName [XQuery §4.1] [XML 1.0 §4.3.3 [81]]");
                return Err(xquery_error(ErrorCode::XQST0087, self.lexer.place(encoding_at), &why));
            }
            Some(encoding)
        } else {
            None
        };
        self.expect_sym(Sym::Semicolon, rule)?;
        Ok(VersionDecl { version, encoding })
    }

    /// `ModuleDecl` [XQuery Appendix A.1 \[5\]], which `module namespace`
    /// begins.
    fn module_decl(&mut self, declared: &mut Declared) -> Result<ModuleDecl> {
        let rule = "a module declaration is 'module namespace', a prefix, '=', a URI literal, then ';' [XQuery Appendix A.1 [5]]";
        self.ahead.pop_front();
        self.ahead.pop_front();
        let at = self.here();
        let prefix = self.expect_ncname(rule)?;
        self.expect_sym(Sym::Eq, rule)?;
        let uri = self.expect_string(rule)?;
        self.declare_prefix(declared, &prefix, &uri, at)?;
        self.expect_sym(Sym::Semicolon, rule)?;
        Ok(ModuleDecl { prefix, uri })
    }

    /// `Prolog` [XQuery Appendix A.1 \[6\]]: setters, namespace
    /// declarations and imports, then variable, function and option
    /// declarations, each followed by its `Separator` \[9\].
    fn prolog(&mut self, mut declared: Declared) -> Result<Prolog> {
        let mut prolog = Prolog::default();
        loop {
            let at = self.here();
            if let Some(setup) = self.setup()? {
                if !prolog.declarations.is_empty() {
                    return Err(self.lexer.error(
                        at,
                        "a prolog's setters, namespace declarations and imports come before its variable, function and option declarations [XQuery Appendix A.1 [6]]",
                    ));
                }
                self.check_setup(&mut declared, &setup, at)?;
                prolog.setup.push(setup);
            } else if let Some(declaration) = self.declaration()? {
                prolog.declarations.push(declaration);
            } else {
                return Ok(prolog);
            }
            self.expect_sym(Sym::Semicolon, "each declaration of a prolog ends with ';' [XQuery Appendix A.1 [6], [9]]")?;
        }
    }

    /// The static errors `setup`, at `at`, raises beside what the prolog
    /// declared before it, `declared`, to which it is added.
    fn check_setup(&self, declared: &mut Declared, setup: &Setup, at: usize) -> Result<()> {
        if let Some((what, code)) = once(setup) {
            if declared.once.contains(&what) {
                let why = format!("a prolog has one {what} declaration at most [XQuery §4]");
                return Err(xquery_error(code, self.lexer.place(at), &why));
            }
            declared.once.push(what);
        }
        match setup {
            Setup::Namespace(decl) => self.declare_prefix(declared, &decl.prefix, &decl.uri, at),
            Setup::Import(import) => match declarations::import_prefix(import) {
                Some((prefix, uri)) => self.declare_prefix(declared, prefix, uri, at),
                None => Ok(()),
            },
            Setup::DefaultNamespace(_) | Setup::Setter(_) => Ok(()),
        }
    }

    /// Adds the prefix `prefix`, bound to `uri` at `at`, to those
    /// `declared`: each prefix once \[err:XQST0033\], never `xml` or
    /// `xmlns`, and no prefix bound to `xml`'s namespace \[err:XQST0070\]
    /// [XQuery §4.12].
    fn declare_prefix(&self, declared: &mut Declared, prefix: &str, uri: &str, at: usize) -> Result<()> {
        let place = self.lexer.place(at);
        if prefix == XML_PREFIX || prefix == XMLNS_PREFIX || uri == XML_NAMESPACE {
            let why = "a prolog declares no prefix 'xml' or 'xmlns', and binds no prefix to the namespace of 'xml' [XQuery §4.12]";
            return Err(xquery_error(ErrorCode::XQST0070, place, why));
        }
        if declared.prefixes.iter().any(|p| p == prefix) {
            let why = format!("a prolog declares the prefix '{prefix}' more than once [XQuery §4.12]");
            return Err(xquery_error(ErrorCode::XQST0033, place, &why));
        }
        declared.prefixes.push(prefix.to_owned());
        Ok(())
    }
}

/// What `setup` declares that a prolog declares once at most, and the
/// error of declaring it twice [XQuery §4.3 to §4.9, §4.13].
fn once(setup: &Setup) -> Option<(&'static str, ErrorCode)> {
    match setup {
        Setup::DefaultNamespace(decl) => Some(match decl.kind {
            NamespaceKind::Element => ("default element/type namespace", ErrorCode::XQST0066),
            NamespaceKind::Function => ("default function namespace", ErrorCode::XQST0066),
        }),
        Setup::Setter(setter) => Some(match setter {
            Setter::BoundarySpace(_) => ("boundary-space", ErrorCode::XQST0068),
            Setter::DefaultCollation(_) => ("default collation", ErrorCode::XQST0038),
            Setter::BaseUri(_) => ("base URI", ErrorCode::XQST0032),
            Setter::Construction(_) => ("construction", ErrorCode::XQST0067),
            Setter::OrderingMode(_) => ("ordering mode", ErrorCode::XQST0065),
            Setter::EmptyOrder(_) => ("empty order", ErrorCode::XQST0069),
            Setter::CopyNamespaces(..) => ("copy-namespaces", ErrorCode::XQST0055),
        }),
        Setup::Namespace(_) | Setup::Import(_) => None,
    }
}

/// Whether `name` is an `EncName` [XML 1.0 §4.3.3 \[81\]]: a Latin letter,
/// then Latin letters, digits, `.`, `_` and `-`.
fn is_enc_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests;
