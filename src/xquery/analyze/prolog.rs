//! A prolog analysed [XQuery §4]: its setters, namespace declarations,
//! default namespace declarations and imports changing the static context
//! in order [XQuery §4.3 to §4.13], then its variables, functions and
//! options declared [XQuery §4.14 to §4.16].
//!
//! An initializing expression, and a function body, sees every function
//! the prolog declares or imports and only the variables declared or
//! imported before it [XQuery §4.14, §4.15]; so a variable's initializing
//! expression depends on the variable only through functions, which the
//! graph of what each reads and calls shows \[err:XQST0054\]
//! (`analyze/linked.rs`). A library module's functions and variables are
//! in its target namespace \[err:XQST0048\] [XQuery §4.2].

use std::sync::Arc;

use super::super::ast::{
    BoundarySpace, Construction, Declaration, EmptyOrder, FunctionDecl, Import, InheritMode, NamespaceKind, OptionDecl, Param,
    PreserveMode, Setter, Setup, VarDecl,
};
use super::super::collation::Collator;
use super::super::error::{ErrorCode, error};
use super::super::functions::FN_NAMESPACE;
use super::super::plan::{Body, Function, Global, GlobalValue, Implementation, Op};
use super::super::types::{Defined, ItemKind, NodeKind, SchemaType, SeqType, XDT_NAMESPACE};
use super::linked::{Exports, Reads};
use super::scope::{Frame, Unprefixed, Var, display};
use super::{Analysis, OPTION_NAMESPACE, STATIC_TYPING_OPTION};
use crate::error::{Code, Result, SqlError};
use crate::xml::{QName, XML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE};

/// [XQuery §4.16] The values of turborel's option `static-typing`.
const ON: &str = "on";
/// See [`ON`].
const OFF: &str = "off";

/// [XQuery §4.15] The namespaces no declared function is in
/// \[err:XQST0045\].
const RESERVED: [&str; 5] = [XML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE, FN_NAMESPACE, XDT_NAMESPACE];

impl Analysis<'_> {
    /// The first part of a prolog applied to the static context, in order
    /// [XQuery §4]; the parser has refused a setter declared twice. Once
    /// it is, what it imports is noted, so the schema types the modules
    /// imported declare their functions and variables of are checked to be
    /// in its in-scope schema types \[err:XQST0036\].
    pub(super) fn setup(&mut self, setup: &[Setup]) -> Result<()> {
        for item in setup {
            match item {
                Setup::Namespace(decl) => self.scope.names.prefixes.push((decl.prefix.as_str().into(), decl.uri.as_str().into())),
                Setup::DefaultNamespace(decl) => {
                    // [XQuery §4.13] the zero-length string is "none".
                    let uri: Option<Box<str>> = (!decl.uri.is_empty()).then(|| decl.uri.as_str().into());
                    match decl.kind {
                        NamespaceKind::Element => self.scope.names.default_element = uri,
                        NamespaceKind::Function => self.scope.names.default_function = uri,
                    }
                }
                Setup::Setter(setter) => self.setter(setter)?,
                Setup::Import(Import::Schema(import)) => self.import_schema(import)?,
                Setup::Import(Import::Module(import)) => self.import_module(import)?,
            }
        }
        self.imported_types();
        Ok(())
    }

    /// `setter` applied [XQuery §4.3 to §4.9].
    fn setter(&mut self, setter: &Setter) -> Result<()> {
        match setter {
            Setter::BoundarySpace(policy) => self.boundary_preserve = *policy == BoundarySpace::Preserve,
            Setter::DefaultCollation(uri) => {
                // [XQuery §4.4] a collation of the statically known ones.
                let Some(collation) = Collator::named(uri, self.env.collations) else {
                    return Err(error(ErrorCode::XQST0038, &format!("no statically known collation is '{uri}' [XQuery §4.4]")));
                };
                self.settings.collation = collation;
            }
            Setter::BaseUri(uri) => self.settings.base_uri = Some(uri.as_str().into()),
            Setter::Construction(mode) => self.settings.preserve = *mode == Construction::Preserve,
            // [XQuery §3.9] turborel's results are in the order ordered
            // gives in either mode, as unordered allows any.
            Setter::OrderingMode(_) => {}
            Setter::EmptyOrder(order) => self.empty_greatest = *order == EmptyOrder::Greatest,
            Setter::CopyNamespaces(preserve, inherit) => {
                self.settings.copy_preserve = *preserve == PreserveMode::Preserve;
                self.settings.copy_inherit = *inherit == InheritMode::Inherit;
            }
        }
        Ok(())
    }

    /// The second part of a prolog [XQuery §4.14 to §4.16]: the functions
    /// declared first, as every body and initializing expression sees all
    /// of them; then each declaration in order. What the module declares
    /// itself, which a library module exports [XQuery §4.11].
    pub(super) fn declarations(&mut self, declarations: &[Declaration]) -> Result<Exports> {
        let mut own = Exports::default();
        for declaration in declarations {
            if let Declaration::Function(f) = declaration {
                let function = self.signature(f)?;
                function.parameters.iter().chain([&function.result]).for_each(|t| defined_types(t, &mut own.types));
                let (name, arity) = (function.name.clone(), function.parameters.len());
                let place = self.linked.function(self.reserved.functions.pop_front().flatten(), function);
                self.declared.push((name.clone(), arity, place));
                own.functions.push((name, arity, place));
            }
        }
        let mut bodies = own.functions.iter().map(|(_, _, place)| *place).collect::<Vec<_>>().into_iter();
        for declaration in declarations {
            match declaration {
                Declaration::Variable(v) => {
                    let (global, read) = self.variable(v)?;
                    global.declared.iter().for_each(|t| defined_types(t, &mut own.types));
                    let name = global.name.clone();
                    let place = self.linked.global(self.reserved.globals.pop_front().flatten(), global, read);
                    self.globals.push((name.clone(), place));
                    own.globals.push((name, place));
                }
                Declaration::Function(f) => {
                    let place = bodies.next();
                    if let (Some(place), Some((body, read))) = (place, self.body(f)?) {
                        self.linked.body(place, body, read);
                    }
                }
                Declaration::Option(option) => self.option(option)?,
            }
        }
        Ok(own)
    }

    /// [XQuery §4.2, §4.14, §4.15] The error if `name`, of a function or
    /// variable a library module declares, is not in its target namespace
    /// \[err:XQST0048\].
    fn in_target(&self, name: &QName, what: &str) -> Result<()> {
        let Some(target) = self.own.library.and_then(super::LibraryModule::namespace) else { return Ok(()) };
        if name.uri.as_deref() == Some(target) {
            return Ok(());
        }
        let why = format!("the {what} {} is not in the target namespace {target} of its library module [XQuery §4.2]", display(name));
        Err(error(ErrorCode::XQST0048, &why))
    }

    /// An option declaration [XQuery §4.16], its name resolved: of
    /// turborel's namespace of options, `static-typing` `on` puts the
    /// Static Typing Feature in effect for the query, `off` leaves it as
    /// the statement has it, as an option suppresses no static error; an
    /// option of another namespace is ignored.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of an option of turborel's namespace it does not
    /// recognize, or of another value.
    fn option(&mut self, option: &OptionDecl) -> Result<()> {
        let name = self.scope.name(&option.name, Unprefixed::None)?;
        if name.uri.as_deref() != Some(OPTION_NAMESPACE) {
            return Ok(());
        }
        let local = &*name.local;
        let value = option.value.as_str();
        if local == STATIC_TYPING_OPTION && value == ON {
            self.typing = super::Typing::Static;
        } else if local != STATIC_TYPING_OPTION || value != OFF {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!(
                    "[EXTENSION] turborel has no option {local} '{value}' of {OPTION_NAMESPACE}: {STATIC_TYPING_OPTION} takes '{ON}' or '{OFF}' [XQuery §4.16]"
                ),
            ));
        }
        Ok(())
    }

    /// The function `f` declares, its body not yet compiled [XQuery §4.15]:
    /// in a namespace \[err:XQST0060\], a library module's target namespace
    /// \[err:XQST0048\], none reserved \[err:XQST0045\], no other of its
    /// name and arity declared or imported \[err:XQST0034\]; an external
    /// one the library's of its name and arity.
    fn signature(&mut self, f: &FunctionDecl) -> Result<Function> {
        let name = self.scope.name(&f.name, Unprefixed::Function)?;
        let Some(uri) = name.uri.as_deref() else {
            return Err(error(ErrorCode::XQST0060, &format!("the function {} is declared in no namespace [XQuery §4.15]", f.name)));
        };
        self.in_target(&name, "function")?;
        if RESERVED.contains(&uri) {
            return Err(error(
                ErrorCode::XQST0045,
                &format!("the function {} is declared in the reserved namespace {uri} [XQuery §4.15]", f.name),
            ));
        }
        let arity = f.params.len();
        if self.declared.iter().any(|(n, a, _)| n.same(&name) && *a == arity) || self.constructor(&name, arity) {
            let why = format!("the function {} of {arity} parameters is declared twice [XQuery §4.15, §4.11]", display(&name));
            return Err(error(ErrorCode::XQST0034, &why));
        }
        let parameters = f.params.iter().map(|p| self.declared_type(p.declared.as_ref())).collect::<Result<Vec<_>>>()?;
        let result = self.declared_type(f.returns.as_ref())?;
        let implementation = if f.body.is_some() {
            Implementation::Body(Body { op: Op::Empty, slots: 0 })
        } else if let Some(found) = self.env.library.find(&name, arity) {
            Implementation::External(found)
        } else {
            let why = format!("no external function {} of {arity} parameters is registered [XQuery §4.15]", display(&name));
            return Err(error(ErrorCode::XPST0017, &why));
        };
        Ok(Function { name, parameters, result, implementation, module: self.module })
    }

    /// The type of what declares `declared`, `item()*` of none [XQuery
    /// §4.15].
    fn declared_type(&self, declared: Option<&super::super::ast::SequenceType>) -> Result<SeqType> {
        declared.map_or(Ok(SeqType::ANY), |t| self.sequence_type(t))
    }

    /// The body of the function `f` compiled, its parameters the first
    /// slots of its frame, and what it reads and calls; none of an
    /// external function.
    fn body(&mut self, f: &FunctionDecl) -> Result<Option<(Body, Reads)>> {
        let Some(body) = &f.body else { return Ok(None) };
        let params = f.params.iter().map(|p: &Param| self.scope.name(&p.name, Unprefixed::None)).collect::<Result<Vec<_>>>()?;
        let (compiled, frame) = self.frame(&params, |a| a.expr(body))?;
        Ok(Some((compiled, Reads { globals: frame.globals_read, functions: frame.functions_called })))
    }

    /// The variable `v` declares [XQuery §4.14], of a library module in
    /// its target namespace \[err:XQST0048\], no other of its name in scope
    /// \[err:XQST0049\]; what its initializing expression reads and calls.
    fn variable(&mut self, v: &VarDecl) -> Result<(Global, Option<Reads>)> {
        let name = self.scope.name(&v.name, Unprefixed::None)?;
        self.in_target(&name, "variable")?;
        if self.globals.iter().any(|(g, _)| g.same(&name)) {
            return Err(error(ErrorCode::XQST0049, &format!("the variable ${} is declared twice [XQuery §4.14, §4.11]", display(&name))));
        }
        let declared = v.declared.as_ref().map(|t| self.sequence_type(t)).transpose()?;
        let (value, read) = match &v.value {
            Some(init) => {
                let (body, frame) = self.frame(&[], |a| a.expr_single(init))?;
                (GlobalValue::Initialized(body), Some(Reads { globals: frame.globals_read, functions: frame.functions_called }))
            }
            None => (GlobalValue::Missing, None),
        };
        Ok((Global { name, declared, value, module: self.module }, read))
    }

    /// `compile` run in a new frame: the globals in scope so far, and
    /// `params` bound to its first slots; its body and what it read and
    /// called.
    pub(super) fn frame(&mut self, params: &[QName], compile: impl FnOnce(&mut Self) -> Result<Op>) -> Result<(Body, Frame)> {
        let mut variables: Vec<(QName, Var)> = self.globals.iter().map(|(g, at)| (g.clone(), Var::Global(*at))).collect();
        let saved = self.scope.begin_frame(Vec::new());
        for name in params {
            let slot = self.scope.slot();
            variables.push((name.clone(), Var::Local(slot)));
        }
        for (name, var) in variables {
            self.scope.bind(name, var);
        }
        let compiled = compile(self);
        let frame = self.scope.end_frame(saved);
        Ok((Body { op: compiled?, slots: frame.slots }, frame))
    }
}

/// Adds to `into` each type definition of an imported schema `t` names:
/// as a user-defined atomic type, or the type of an element's or an
/// attribute's annotation; each once.
fn defined_types(t: &SeqType, into: &mut Vec<Arc<Defined>>) {
    let SeqType::Of(item, _) = t else { return };
    let annotation = match item {
        ItemKind::Atomic(atomic) => Some(atomic),
        ItemKind::Node(kind) => node_annotation(kind),
        ItemKind::Item | ItemKind::Numeric => None,
    };
    if let Some(SchemaType::Defined(d)) = annotation
        && !into.iter().any(|known| known == d)
    {
        into.push(Arc::clone(d));
    }
}

/// The type an element's or an attribute's annotation `kind` tests
/// derives from, if it tests one.
fn node_annotation(kind: &NodeKind) -> Option<&SchemaType> {
    match kind {
        NodeKind::Element(e) | NodeKind::Document(Some(e)) => e.annotation.as_ref(),
        NodeKind::Attribute(a) => a.annotation.as_ref(),
        NodeKind::Any | NodeKind::Document(None) | NodeKind::Pi(_) | NodeKind::Comment | NodeKind::Text => None,
    }
}
