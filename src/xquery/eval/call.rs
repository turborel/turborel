//! Function calls and the variables of the prolog, evaluated [XQuery
//! §3.1.5, §4.14]: a call's arguments converted to its parameters' types,
//! a declared function's body evaluated in a frame of its own with no
//! focus, a library function called with the call's dynamic context, and
//! the result converted to the result type; a variable of the prolog
//! evaluated once, as it is first read. A declared function's body, and a
//! variable's initializing expression, are evaluated in the static
//! context of the module that declares it [XQuery §4.11].

use std::rc::Rc;

use super::super::error::{ErrorCode, error};
use super::super::functions::{Context, QueryContext, XQueryFunction};
use super::super::plan::{CallOp, Callee, GlobalValue, Implementation};
use super::super::types::SeqType;
use super::types::converted;
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::Result;
use crate::xml::Sequence;

impl Evaluator<'_> {
    /// A function call [XQuery §3.1.5].
    pub(super) fn call(&self, c: &CallOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut arguments = Vec::with_capacity(c.arguments.len());
        for argument in &c.arguments {
            arguments.push(self.eval(argument, frame, focus)?);
        }
        let _deeper = self.deeper()?;
        self.check()?;
        match &c.callee {
            Callee::Library(function) => self.library_call(&**function, arguments, focus),
            Callee::Declared(at) => self.declared_call(*at, arguments),
        }
    }

    /// The call of the `at`th function of the prolog with `arguments`: its
    /// body in a frame of its own, the focus undefined, or the external
    /// function's.
    fn declared_call(&self, at: usize, arguments: Vec<Seq>) -> Result<Seq> {
        let Some(function) = self.plan.functions.get(at) else { return Err(missing("function")) };
        let mut values = Vec::with_capacity(arguments.len());
        let what = format!("an argument of {}", function.name.local);
        for (argument, t) in arguments.into_iter().zip(&function.parameters) {
            values.push(converted(argument, t, &what)?);
        }
        let _within = self.within(function.module);
        let result = match &function.implementation {
            Implementation::Body(body) => {
                let mut own = Frame::new(body.slots);
                for (slot, value) in values.into_iter().enumerate() {
                    own.set(slot, Rc::new(value));
                }
                self.eval(&body.op, &mut own, &Focus::default())?
            }
            Implementation::External(external) => self.library_call(&**external, values, &Focus::default())?,
        };
        converted(result, &function.result, &format!("the result of {}", function.name.local))
    }

    /// The call of `function` of the library with `arguments`, in the
    /// dynamic context of `focus`, its arguments and result converted to
    /// its types.
    fn library_call(&self, function: &dyn XQueryFunction, arguments: Vec<Seq>, focus: &Focus) -> Result<Seq> {
        let mut values = Vec::with_capacity(arguments.len());
        let types = function.parameters();
        for (n, argument) in arguments.into_iter().enumerate() {
            // [XQueryFO §7.4.1] an argument past a variadic function's
            // parameters is of its last's type.
            let t = types.get(n).or_else(|| types.last()).unwrap_or(&SeqType::ANY);
            values.push(Sequence::new(converted(argument, t, "an argument of a function")?));
        }
        let settings = &self.module_context().settings;
        let query = QueryContext {
            now: self.context.now,
            timezone: self.context.timezone,
            collation: &settings.collation,
            collations: self.context.collations,
            base_uri: settings.base_uri.as_deref(),
            interrupted: self.context.interrupted,
        };
        let context = Context { item: focus.item.as_ref(), position: focus.position, size: focus.size, query: &query };
        let result = function.call(&context, values)?.into_items();
        super::sequence::bounded(result.len())?;
        converted(result, function.result(), "the result of a function")
    }

    /// The value of the `at`th global [XQuery §4.14]: a variable passed in,
    /// or one of the prolog, its initializing expression evaluated as it is
    /// first read, with the query's focus; of the declared type, if any
    /// \[err:XPTY0004\]; an external variable passed no value err:XPDY0002.
    pub(super) fn global(&self, at: usize) -> Result<Rc<Seq>> {
        if let Some(Some(value)) = self.globals.borrow().get(at) {
            return Ok(Rc::clone(value));
        }
        let Some(global) = self.plan.globals.get(at) else { return Err(missing("variable")) };
        let _within = self.within(global.module);
        let value = match &global.value {
            GlobalValue::Passed(n) => self.passed.get(*n).cloned().unwrap_or_default(),
            GlobalValue::Initialized(body) => {
                let mut own = Frame::new(body.slots);
                Rc::new(self.eval(&body.op, &mut own, &self.initial)?)
            }
            GlobalValue::Missing => {
                let why = format!("the external variable ${} has no value [XQuery §4.14]", global.name.local);
                return Err(error(ErrorCode::XPDY0002, &why));
            }
        };
        if !global.declared.as_ref().is_none_or(|t: &SeqType| t.matches(&value)) {
            let why = format!("the value of ${} does not match its declared type [XQuery §4.14]", global.name.local);
            return Err(error(ErrorCode::XPTY0004, &why));
        }
        if let Some(slot) = self.globals.borrow_mut().get_mut(at) {
            *slot = Some(Rc::clone(&value));
        }
        Ok(value)
    }
}

/// The error of a plan naming what it does not have, which no compiled
/// plan does (SQLCODE -910).
fn missing(what: &str) -> crate::error::SqlError {
    crate::error::SqlError::new(crate::error::Code::Api, format!("[DESIGN] a plan calls or reads a {what} it does not have"))
}
