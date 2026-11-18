//! What every module of a query declares, as its modules are analysed:
//! one list of the variables and one of the functions of them all [XQuery
//! §4.11, §4.14, §4.15], so a reference or a call is a place in it
//! whichever module declares what it names; the static context of each
//! module the evaluation reads; what each library module analysed exports
//! to those importing it; and the graph of what each initializing
//! expression and function body reads and calls, of every module, by which
//! a circularity is found \[err:XQST0054\].

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::error::{ErrorCode, beyond, error};
use super::super::plan::{Body, Function, Global, Implementation, ModuleContext, Settings};
use super::super::types::Defined;
use super::scope::display;
use crate::error::Result;
use crate::xml::QName;

/// \[DESIGN\] How many module imports lead at most from a query's main
/// module to a library module it imports (RULES.md R-CF-6, D106): each is
/// analysed within the analysis of the module importing it. \[XQuery\]
/// sets no limit, so a chain longer is SQLCODE -900 (D18), as the module
/// importing it is registered. The value is no measurement: it is past
/// the layers of libraries an application builds, and each level holds
/// only an analysis.
pub const MAX_IMPORT_DEPTH: usize = 16;

/// A function signature of a module [XQuery §4.15]: the function's
/// expanded QName, its arity and its place among the query's functions.
pub(super) type Signature = (QName, usize, usize);

/// What a variable or function of a prolog reads and calls, for the graph
/// of dependencies \[err:XQST0054\].
#[derive(Default)]
pub(super) struct Reads {
    /// The globals its expression reads, by place.
    pub(super) globals: BTreeSet<usize>,
    /// The declared functions it calls, by place.
    pub(super) functions: BTreeSet<usize>,
}

/// What a library module analysed exports to the modules importing it
/// [XQuery §4.11]: the functions and variables it declares itself, not
/// those it imports, as imports are not transitive; and the schema types
/// their declarations name \[err:XQST0036\].
#[derive(Clone, Default)]
pub(super) struct Exports {
    /// Its functions' signatures.
    pub(super) functions: Vec<Signature>,
    /// Its variables, each name and place.
    pub(super) globals: Vec<(QName, usize)>,
    /// The type definitions of imported schemas its functions' parameters
    /// and results and its variables are declared of.
    pub(super) types: Vec<Arc<Defined>>,
}

/// What the modules of a query analysed so far declare.
#[derive(Default)]
pub(super) struct Linked {
    /// Every module's variables, those passed in among them.
    globals: Vec<Global>,
    /// What each global's initializing expression reads, none of one
    /// passed in or external.
    reads: Vec<Option<Reads>>,
    /// Every module's functions, each declared before its body is
    /// compiled.
    functions: Vec<Function>,
    /// What each function's body reads and calls.
    calls: Vec<Reads>,
    /// Each module's static context, as `Plan::context` numbers them: the
    /// main module's, at 0, made apart.
    contexts: Vec<ModuleContext>,
    /// What each library module analysed exports, by its name.
    exported: Vec<(String, Exports)>,
    /// The library modules being analysed, each importing the next.
    open: Vec<String>,
}

impl Linked {
    /// A module begun: its place, as `Plan::context` numbers them.
    pub(super) fn begin(&mut self) -> usize {
        let at = self.contexts.len();
        self.contexts.push(ModuleContext { settings: Settings::initial(), schemas: Vec::new() });
        at
    }

    /// The static context of the `module`th module, once analysed.
    pub(super) fn end(&mut self, module: usize, context: ModuleContext) {
        if let Some(slot) = self.contexts.get_mut(module) {
            *slot = context;
        }
    }

    /// `global` added, of what its initializing expression `read` reads:
    /// its place.
    pub(super) fn global(&mut self, global: Global, read: Option<Reads>) -> usize {
        self.globals.push(global);
        self.reads.push(read);
        self.globals.len().saturating_sub(1)
    }

    /// `function` added, its body not yet compiled: its place.
    pub(super) fn function(&mut self, function: Function) -> usize {
        self.functions.push(function);
        self.calls.push(Reads::default());
        self.functions.len().saturating_sub(1)
    }

    /// The body of the function at `at` compiled, and what it `read`s and
    /// calls.
    pub(super) fn body(&mut self, at: usize, body: Body, read: Reads) {
        if let Some(function) = self.functions.get_mut(at) {
            function.implementation = Implementation::Body(body);
        }
        if let Some(slot) = self.calls.get_mut(at) {
            *slot = read;
        }
    }

    /// What the library module `name` exports, if it has been analysed.
    pub(super) fn exports(&self, name: &str) -> Option<&Exports> {
        self.exported.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }

    /// The analysis of the library module `name` begun, `depth` imports
    /// from the main module.
    ///
    /// # Errors
    ///
    /// err:XQST0073 if it is being analysed already, as it imports itself
    /// through those it imports, which no registered XQuery module does, as
    /// each imports those registered before it alone; SQLCODE -900 if
    /// `depth` is past `MAX_IMPORT_DEPTH`.
    pub(super) fn open(&mut self, name: &str, depth: usize) -> Result<()> {
        if self.open.iter().any(|n| n == name) {
            let why = format!("the module {name} imports itself through the modules it imports [XQuery §4.11]");
            return Err(error(ErrorCode::XQST0073, &why));
        }
        if depth > MAX_IMPORT_DEPTH {
            return Err(beyond(&format!("modules import modules more than {MAX_IMPORT_DEPTH} deep")));
        }
        self.open.push(name.to_owned());
        Ok(())
    }

    /// The analysis of the library module `name` ended, what it exports
    /// `exports`.
    pub(super) fn close(&mut self, name: &str, exports: Exports) {
        self.open.retain(|n| n != name);
        self.exported.push((name.to_owned(), exports));
    }

    /// [XQuery §4.14] The error if a variable's initializing expression
    /// depends on the variable itself, through the functions it calls,
    /// those they call, and the variables they read, of any module
    /// \[err:XQST0054\].
    pub(super) fn circularity(&self) -> Result<()> {
        for (at, global) in self.globals.iter().enumerate() {
            let Some(Some(start)) = self.reads.get(at) else { continue };
            if self.reaches(start, at) {
                let why = format!("the initializing expression of ${} depends on it [XQuery §4.14]", display(&global.name));
                return Err(error(ErrorCode::XQST0054, &why));
            }
        }
        Ok(())
    }

    /// Whether what `start` reads and calls reads the `target`th global, at
    /// any remove: a walk of the graph, each global and function once, so
    /// it ends.
    fn reaches(&self, start: &Reads, target: usize) -> bool {
        let (mut seen_globals, mut seen_functions) = (BTreeSet::new(), BTreeSet::new());
        let mut left: Vec<&Reads> = vec![start];
        while let Some(next) = left.pop() {
            if next.globals.contains(&target) {
                return true;
            }
            let globals =
                next.globals.iter().filter(|g| seen_globals.insert(**g)).filter_map(|g| self.reads.get(*g).and_then(Option::as_ref));
            let functions = next.functions.iter().filter(|f| seen_functions.insert(**f)).filter_map(|f| self.calls.get(*f));
            let more: Vec<&Reads> = globals.chain(functions).collect();
            left.extend(more);
        }
        false
    }

    /// Its globals, functions and the library modules' static contexts,
    /// for the plan.
    pub(super) fn into_parts(self) -> (Vec<Global>, Vec<Function>, Vec<ModuleContext>) {
        (self.globals, self.functions, self.contexts.into_iter().skip(1).collect())
    }
}
