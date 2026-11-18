//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.56, §5.1.58, §5.1.60,
//! §7.6.4, §7.6.5, §7.8.5, §7.8.6, §7.9.4] Two routines a method's name
//! and arity bind to. A method of one argument after SELF that is both
//! an observer of a unit name and a mutator of a number, as
//! ST_UAxisLength(unit) and ST_UAxisLength(DOUBLE PRECISION), is the one
//! its argument's type chooses (`Either`); and the methods ST_WKTToSQL,
//! ST_WKBToSQL and ST_GMLToSQL are their functions of the arguments
//! after SELF (`WithoutSelf`).

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::error::Result;
use crate::functions::{AppendedQuery, CallContext, ScalarFunction};
use crate::value::{DataType, Value};

/// An observer of a character argument, else a mutator.
#[derive(Clone)]
pub struct Either {
    /// The routine of a character argument, a unit name.
    pub text: Arc<dyn ScalarFunction>,
    /// The routine of any other.
    pub number: Arc<dyn ScalarFunction>,
}

impl Either {
    /// The routine of the argument types `types`, SELF's first.
    fn of_types(&self, types: &[DataType]) -> &dyn ScalarFunction {
        if types.get(1).is_some_and(DataType::is_character) { &*self.text } else { &*self.number }
    }

    /// The routine of the arguments `args`, SELF first.
    fn of_values(&self, args: &[Value]) -> &dyn ScalarFunction {
        if matches!(args.get(1), Some(Value::Char(_))) { &*self.text } else { &*self.number }
    }
}

impl ScalarFunction for Either {
    fn name(&self) -> &str {
        self.number.name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.of_types(args).return_type(args)
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        let text = known.get(1).cloned().flatten().is_some_and(|t| t.is_character());
        if text { self.text.argument_type(position, known) } else { self.number.argument_type(position, known) }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.of_values(args).call(args)
    }

    fn uses_context(&self, arity: usize) -> bool {
        self.text.uses_context(arity) || self.number.uses_context(arity)
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        self.of_values(args).call_in(args, cx)
    }

    fn deterministic(&self) -> bool {
        true
    }
}

/// A function as a method: SELF is not its argument.
#[derive(Clone)]
pub struct WithoutSelf(pub Arc<dyn ScalarFunction>);

impl ScalarFunction for WithoutSelf {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.0.return_type(args.get(1..).unwrap_or_default())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        let Some(after) = position.checked_sub(1) else {
            return Some(known.first().cloned().flatten().unwrap_or_else(crate::geometry::kind::data_type));
        };
        self.0.argument_type(after, known.get(1..).unwrap_or_default())
    }

    fn appended_query(&self, constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        self.0.appended_query(constants.get(1..).unwrap_or_default())
    }

    fn catalog_query(&self, constants: &[Option<&Value>], types: &[DataType], catalog: &dyn Catalog) -> Option<AppendedQuery> {
        self.0.catalog_query(constants.get(1..).unwrap_or_default(), types.get(1..).unwrap_or_default(), catalog)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [first, rest @ ..] = args else { return self.0.call(args) };
        if first.is_null() {
            return Ok(Value::Null);
        }
        self.0.call(rest)
    }

    fn deterministic(&self) -> bool {
        true
    }
}
