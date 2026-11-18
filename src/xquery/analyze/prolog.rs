//! A prolog analysed [XQuery §4]: its setters, namespace declarations and
//! default namespace declarations changing the static context in order
//! [XQuery §4.3 to §4.13], then its variables, functions and options
//! declared [XQuery §4.14 to §4.16].
//!
//! An initializing expression, and a function body, sees every function
//! the prolog declares and only the variables declared before it [XQuery
//! §4.14, §4.15]; so a variable's initializing expression depends on the
//! variable only through functions, which the graph of what each reads and
//! calls shows \[err:XQST0054\].

use std::collections::BTreeSet;

use super::super::ast::{
    BoundarySpace, Construction, Declaration, EmptyOrder, FunctionDecl, InheritMode, NamespaceKind, Param, PreserveMode, Setter, Setup,
    VarDecl,
};
use super::super::collation::Collator;
use super::super::error::{ErrorCode, error};
use super::super::functions::FN_NAMESPACE;
use super::super::plan::{Body, Function, Global, GlobalValue, Implementation, Op};
use super::super::types::{SeqType, XDT_NAMESPACE};
use super::Analysis;
use super::scope::{Frame, Unprefixed, Var, display};
use crate::error::Result;
use crate::xml::{QName, XML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE};

/// [XQuery §4.15] The namespaces no declared function is in
/// \[err:XQST0045\].
const RESERVED: [&str; 5] = [XML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE, FN_NAMESPACE, XDT_NAMESPACE];

/// What a variable or function of the prolog read and called, for the
/// graph of dependencies \[err:XQST0054\].
#[derive(Default)]
struct Reads {
    /// The globals its expression reads.
    globals: BTreeSet<usize>,
    /// The declared functions it calls.
    functions: BTreeSet<usize>,
}

/// What the second part of a prolog declares, as it is analysed.
struct Declared {
    /// The globals, those passed in first.
    globals: Vec<Global>,
    /// What each global's initializing expression reads, none of one
    /// passed in or external.
    reads: Vec<Option<Reads>>,
    /// The functions, each declared once its body is compiled.
    functions: Vec<Function>,
    /// What each function's body reads and calls.
    calls: Vec<Reads>,
    /// The place of the next function declared.
    next: usize,
}

impl Analysis<'_> {
    /// The first part of a prolog applied to the static context, in order
    /// [XQuery §4]; the parser has refused a setter declared twice.
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
                Setup::Import(_) => {
                    return Err(error(ErrorCode::XQST0009, "turborel does not support the Schema Import Feature [XQuery §5.2.1]"));
                }
            }
        }
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
    /// of them; then each declaration in order, after `globals`, those
    /// passed in.
    pub(super) fn declarations(&mut self, declarations: &[Declaration], globals: Vec<Global>) -> Result<(Vec<Global>, Vec<Function>)> {
        let mut functions = Vec::new();
        for declaration in declarations {
            if let Declaration::Function(f) = declaration {
                functions.push(self.signature(f)?);
            }
        }
        let calls = functions.iter().map(|_| Reads::default()).collect();
        let reads = globals.iter().map(|_| None).collect();
        let mut declared = Declared { globals, reads, functions, calls, next: 0 };
        for declaration in declarations {
            self.declaration(declaration, &mut declared)?;
        }
        circularity(&declared.globals, &declared.reads, &declared.calls)?;
        Ok((declared.globals, declared.functions))
    }

    /// `declaration` analysed into `declared` [XQuery §4.14 to §4.16].
    fn declaration(&mut self, declaration: &Declaration, declared: &mut Declared) -> Result<()> {
        match declaration {
            Declaration::Variable(v) => {
                let (global, read) = self.variable(v)?;
                declared.reads.push(read);
                declared.globals.push(global);
            }
            Declaration::Function(f) => {
                let at = declared.next;
                declared.next = at.saturating_add(1);
                let Some((body, read)) = self.body(f)? else { return Ok(()) };
                if let Some(function) = declared.functions.get_mut(at) {
                    function.implementation = Implementation::Body(body);
                }
                if let Some(slot) = declared.calls.get_mut(at) {
                    *slot = read;
                }
            }
            Declaration::Option(option) => {
                // [XQuery §4.16] no namespace is recognized as one of
                // options, so each is ignored once its name resolves.
                self.scope.name(&option.name, Unprefixed::None)?;
            }
        }
        Ok(())
    }

    /// The function `f` declares, its body not yet compiled [XQuery §4.15]:
    /// in a namespace \[err:XQST0060\], none reserved \[err:XQST0045\], no
    /// other of its name and arity \[err:XQST0034\]; an external one the
    /// library's of its name and arity.
    fn signature(&mut self, f: &FunctionDecl) -> Result<Function> {
        let name = self.scope.name(&f.name, Unprefixed::Function)?;
        let Some(uri) = name.uri.as_deref() else {
            return Err(error(ErrorCode::XQST0060, &format!("the function {} is declared in no namespace [XQuery §4.15]", f.name)));
        };
        if RESERVED.contains(&uri) {
            return Err(error(
                ErrorCode::XQST0045,
                &format!("the function {} is declared in the reserved namespace {uri} [XQuery §4.15]", f.name),
            ));
        }
        let arity = f.params.len();
        if self.declared.iter().any(|(n, a)| n.same(&name) && *a == arity) || Self::constructor(&name, arity) {
            let why = format!("the function {} of {arity} parameters is declared twice [XQuery §4.15]", display(&name));
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
        self.declared.push((name.clone(), arity));
        Ok(Function { name, parameters, result, implementation })
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

    /// The variable `v` declares [XQuery §4.14], no other of its name in
    /// scope \[err:XQST0049\]; what its initializing expression reads and
    /// calls.
    fn variable(&mut self, v: &VarDecl) -> Result<(Global, Option<Reads>)> {
        let name = self.scope.name(&v.name, Unprefixed::None)?;
        if self.globals.iter().any(|g| g.same(&name)) {
            return Err(error(ErrorCode::XQST0049, &format!("the variable ${} is declared twice [XQuery §4.14]", display(&name))));
        }
        let declared = v.declared.as_ref().map(|t| self.sequence_type(t)).transpose()?;
        let (value, read) = match &v.value {
            Some(init) => {
                let (body, frame) = self.frame(&[], |a| a.expr_single(init))?;
                (GlobalValue::Initialized(body), Some(Reads { globals: frame.globals_read, functions: frame.functions_called }))
            }
            None => (GlobalValue::Missing, None),
        };
        self.globals.push(name.clone());
        Ok((Global { name, declared, value }, read))
    }

    /// `compile` run in a new frame: the globals declared so far in scope,
    /// and `params` bound to its first slots; its body and what it read
    /// and called.
    pub(super) fn frame(&mut self, params: &[QName], compile: impl FnOnce(&mut Self) -> Result<Op>) -> Result<(Body, Frame)> {
        let mut variables: Vec<(QName, Var)> = self.globals.iter().enumerate().map(|(at, g)| (g.clone(), Var::Global(at))).collect();
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

/// [XQuery §4.14] The error if a variable's initializing expression
/// depends on the variable itself, through the functions it calls, those
/// they call, and the variables they read \[err:XQST0054\].
fn circularity(globals: &[Global], reads: &[Option<Reads>], calls: &[Reads]) -> Result<()> {
    for (at, global) in globals.iter().enumerate() {
        let Some(Some(start)) = reads.get(at) else { continue };
        if reaches(start, at, reads, calls) {
            let why = format!("the initializing expression of ${} depends on it [XQuery §4.14]", display(&global.name));
            return Err(error(ErrorCode::XQST0054, &why));
        }
    }
    Ok(())
}

/// Whether what `start` reads and calls reads the `target`th global, at
/// any remove: a walk of the graph, each global and function once, so it
/// ends.
fn reaches(start: &Reads, target: usize, reads: &[Option<Reads>], calls: &[Reads]) -> bool {
    let (mut seen_globals, mut seen_functions) = (BTreeSet::new(), BTreeSet::new());
    let mut left: Vec<&Reads> = vec![start];
    while let Some(next) = left.pop() {
        if next.globals.contains(&target) {
            return true;
        }
        let globals = next.globals.iter().filter(|g| seen_globals.insert(**g)).filter_map(|g| reads.get(*g).and_then(Option::as_ref));
        let functions = next.functions.iter().filter(|f| seen_functions.insert(**f)).filter_map(|f| calls.get(*f));
        let more: Vec<&Reads> = globals.chain(functions).collect();
        left.extend(more);
    }
    false
}
