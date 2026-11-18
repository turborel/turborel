//! \[EXTENSION\] The functions a query calls that it does not declare
//! [XQuery §3.1.5]: the built-in functions of \[XQueryFO\], and functions
//! a plug-in registers (`Database::register_xquery_function`), each an
//! `XQueryFunction` in a `Library`, which an `external` function
//! declaration [XQuery §4.15] and a call by its expanded QName and arity
//! reach.
//!
//! The built-in functions here are those the core of the language relies
//! on (`functions/core.rs`): the boolean functions, fn:data, fn:string,
//! fn:count, fn:empty and fn:exists, fn:position and fn:last, fn:concat,
//! fn:root and fn:doc. The constructor functions of the atomic types are
//! no functions of the library: a call of one is a cast [XQuery §3.12.5].

mod core;

use std::collections::HashMap;
use std::sync::Arc;

use super::collation::{Collations, Collator};
use super::types::SeqType;
use crate::error::{Code, Result, SqlError};
use crate::settings::Configurable;
use crate::xml::{Item, Moment, QName, Sequence};

/// [XQuery §2] The namespace of the built-in functions, `fn`.
pub const FN_NAMESPACE: &str = "http://www.w3.org/2005/xpath-functions";

/// \[EXTENSION\] A function a query may call, built in or a plug-in's
/// (RULES.md D39). Its contract, which every built-in keeps:
///
/// * its name is an expanded QName in a namespace, and with its arity,
///   the number of its parameters, names it alone in a `Library`;
/// * a call is given one sequence for each parameter, already converted
///   to its type by the function conversion rules [XQuery §3.1.5], and
///   the dynamic context of the call (`Context`); what it returns is
///   converted to `result` by the same rules, a value that does not match
///   failing the call \[err:XPTY0004\];
/// * it fails with `xquery::error` of the code \[XQueryFO\] gives, or its
///   own, which SQL/XML raises as SQLSTATE 10000 [ISO/IEC 9075-14:2006
///   draft §6.18 GR3];
/// * it is a function of its arguments and the context it reads, so two
///   calls of one query alike give one result (its stability, [XQueryFO
///   §1.7]); it holds no lock and calls nothing back in the database;
/// * a call that may run long calls `Context::check` as it goes, so the
///   statement running it is cancelled (RULES.md D44), and builds no
///   sequence longer, nor string or tree bigger, than `xml::MAX_NODES`
///   and `xml::MAX_TEXT` allow.
pub trait XQueryFunction: Send + Sync {
    /// Its name, an expanded QName in a namespace [XQuery §2.1.1].
    fn name(&self) -> &QName;
    /// The types of its parameters, one for each: its arity is their
    /// number.
    fn parameters(&self) -> &[SeqType];
    /// The type of its result.
    fn result(&self) -> &SeqType;
    /// Whether it takes any number of arguments from its parameters' on,
    /// each past them of its last parameter's type, as fn:concat does
    /// [XQueryFO §7.4.1]. Default: no.
    fn variadic(&self) -> bool {
        false
    }
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn Configurable> {
        None
    }
    /// The value of a call of it, of `arguments` in the dynamic context
    /// `context` [XQuery §3.1.5].
    ///
    /// # Errors
    ///
    /// The XQuery error \[XQueryFO\] defines for the call, or the
    /// statement's cancellation.
    fn call(&self, context: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence>;
}

/// The dynamic context a function is called in [XQuery §2.1.2, Appendix
/// C.2]: its focus, the current dateTime and implicit timezone, the
/// default collation and the others known, and the base URI.
pub struct Context<'a> {
    /// The context item, if one is defined.
    pub(super) item: Option<&'a Item>,
    /// The context position and size, of a context item.
    pub(super) position: usize,
    /// The context size.
    pub(super) size: usize,
    /// The query's settings and dynamic context.
    pub(super) query: &'a QueryContext<'a>,
}

/// What every call of a query's evaluation shares of its context [XQuery
/// Appendix C].
pub(super) struct QueryContext<'a> {
    /// The current dateTime, with its time zone.
    pub(super) now: Moment,
    /// The implicit timezone, in minutes east of UTC.
    pub(super) timezone: i16,
    /// The default collation.
    pub(super) collation: &'a Collator,
    /// The statically known collations.
    pub(super) collations: &'a Collations,
    /// The base URI, if the static context has one.
    pub(super) base_uri: Option<&'a str>,
    /// What says whether the statement was cancelled.
    pub(super) interrupted: &'a dyn Fn() -> Result<()>,
}

impl Context<'_> {
    /// The context item, if one is defined [XQuery §2.1.2].
    #[must_use]
    pub fn item(&self) -> Option<&Item> {
        self.item
    }

    /// The context position, of a defined context item [XQuery §2.1.2].
    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    /// The context size, of a defined context item [XQuery §2.1.2].
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// The current dateTime [XQuery §2.1.2]: one instant through the
    /// query, with a time zone.
    #[must_use]
    pub fn now(&self) -> Moment {
        self.query.now
    }

    /// The implicit timezone, in minutes east of UTC [XQuery §2.1.2].
    #[must_use]
    pub fn timezone(&self) -> i16 {
        self.query.timezone
    }

    /// The default collation [XQuery §2.1.1].
    #[must_use]
    pub fn collation(&self) -> &Collator {
        self.query.collation
    }

    /// The collation of the statically known collations `uri` names, if
    /// one does [XQuery §2.1.1] [XQueryFO §7.3.1].
    #[must_use]
    pub fn collation_named(&self, uri: &str) -> Option<Collator> {
        Collator::named(uri, self.query.collations)
    }

    /// The base URI of the static context, if it has one [XQuery §2.1.1].
    #[must_use]
    pub fn base_uri(&self) -> Option<&str> {
        self.query.base_uri
    }

    /// Whether the statement evaluating the query was cancelled, or ran
    /// past its time (RULES.md D44).
    ///
    /// # Errors
    ///
    /// SQLCODE -920 if it was.
    pub fn check(&self) -> Result<()> {
        (self.query.interrupted)()
    }
}

/// A function's name in a library: its namespace and local name.
type Key = (Option<Box<str>>, Box<str>);

/// \[EXTENSION\] The functions a query may call by name that it does not
/// declare: the built-in functions and those registered, each of a name
/// and arity alone.
#[derive(Clone)]
pub struct Library {
    functions: HashMap<Key, Vec<Arc<dyn XQueryFunction>>>,
}

impl Default for Library {
    fn default() -> Self {
        Library::builtin()
    }
}

/// Whether `f` takes `arity` arguments: its parameters', or as many or
/// more of a variadic one.
fn takes(f: &dyn XQueryFunction, arity: usize) -> bool {
    let n = f.parameters().len();
    arity == n || (f.variadic() && arity >= n)
}

impl Library {
    /// The built-in functions alone.
    #[must_use]
    pub fn builtin() -> Library {
        let mut library = Library { functions: HashMap::new() };
        for f in core::all() {
            library.functions.entry(key(&*f)).or_default().push(f);
        }
        library
    }

    /// `f` added.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is in no namespace, or a function of its name
    /// takes one of the arities it takes: none is replaced, so no query's
    /// calls change under it.
    pub fn add(&mut self, f: Arc<dyn XQueryFunction>) -> Result<()> {
        let name = f.name();
        if name.uri.as_deref().is_none_or(str::is_empty) {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] the XQuery function {} is in no namespace", name.local)));
        }
        let key = key(&*f);
        let n = f.parameters().len();
        let clashes = |g: &Arc<dyn XQueryFunction>| takes(&**g, n) || takes(&*f, g.parameters().len());
        if self.functions.get(&key).is_some_and(|others| others.iter().any(clashes)) {
            let (uri, local) = key;
            let uri = uri.unwrap_or_default();
            let why = format!("[EXTENSION] an XQuery function {{{uri}}}{local} of {n} parameters is registered already");
            return Err(SqlError::new(Code::Api, why));
        }
        self.functions.entry(key).or_default().push(f);
        Ok(())
    }

    /// The function of the expanded QName `name` taking `arity` arguments,
    /// if the library has one.
    #[must_use]
    pub fn find(&self, name: &QName, arity: usize) -> Option<Arc<dyn XQueryFunction>> {
        let found = self.functions.get(&(name.uri.clone(), name.local.clone()))?;
        found.iter().find(|f| takes(&***f, arity)).cloned()
    }

    /// Its functions' hooks for their settings (RULES.md D36).
    pub fn configurables(&self) -> impl Iterator<Item = &dyn Configurable> {
        self.functions.values().flatten().filter_map(|f| f.configurable())
    }
}

/// The name of `f`.
fn key(f: &dyn XQueryFunction) -> Key {
    let name = f.name();
    (name.uri.clone(), name.local.clone())
}

#[cfg(test)]
mod tests;
