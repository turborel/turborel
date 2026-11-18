//! The routines of §15 of one name as one function: as it is bound, the
//! routine whose parameters the declared types of its arguments fit; as
//! it runs, the routine whose parameters its values fit, the null value
//! fitting any. A function of the name already there, a geometry
//! routine, is invoked of arguments no routine of §15 takes.

use std::borrow::Borrow;
use std::cell::Cell;
use std::sync::{Arc, LazyLock};

use super::super::{Catalog, Unit, linear_units};
use super::args::Args;
use super::catalog::{self, Read};
use super::routine::Routine;
use super::routines;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::{AppendedQuery, CallContext, JsonProbe, ScalarFunction, ScalarRef};
use crate::geometry::SpatialOperation;
use crate::index::expression::KeyArgument;
use crate::memory::MemoryBudget;
use crate::params::ParameterType;
use crate::settings::Configurable;
use crate::value::lob::TempLobs;
use crate::value::{DataType, Value};

/// [§15.1.1 Description 16] The supported units: the LINEAR rows of
/// ST_UNITS_OF_MEASURE, which are those supplied.
static UNITS: LazyLock<Vec<Unit<'static>>> = LazyLock::new(linear_units);

/// The routines of §15 of one name, and the function of the name there
/// was before, if one was.
pub struct LrFunction {
    name: &'static str,
    routines: Vec<&'static Routine>,
    fallback: Option<Arc<dyn ScalarFunction>>,
}

/// Of a call, the routine invoked and whether the catalog's ARRAY was
/// appended to its arguments.
type Chosen = (&'static Routine, bool);

impl LrFunction {
    /// The function of `name`, of the routines of that name, taking
    /// `fallback`'s other arguments.
    pub fn new(name: &'static str, fallback: Option<Arc<dyn ScalarFunction>>) -> LrFunction {
        LrFunction { name, routines: routines::all().filter(|r| r.name == name).collect(), fallback }
    }

    /// It, taking `fallback`'s calls in place of its own fallback's.
    fn over(&self, fallback: Arc<dyn ScalarFunction>) -> LrFunction {
        LrFunction { name: self.name, routines: self.routines.clone(), fallback: Some(fallback) }
    }

    /// The function there was before of the name, if one was and a call
    /// of values `values` is not one of a routine of §15.
    fn other<V: Borrow<Value>>(&self, values: &[V]) -> Option<&Arc<dyn ScalarFunction>> {
        self.fallback.as_ref().filter(|_| self.by_values(values).is_none())
    }

    /// The routine the declared types `types` choose.
    fn by_types(&self, types: &[DataType]) -> Option<Chosen> {
        let own = self.routines.iter().find(|r| r.takes(types)).map(|r| (*r, false));
        own.or_else(|| {
            let (_, given) = types.split_last()?;
            self.routines.iter().find(|r| r.catalog && r.takes(given)).map(|r| (*r, true))
        })
    }

    /// The routine the values `values` choose. Null values alone choose
    /// none of a name the function there was before shares: a null fits
    /// any parameter, so it cannot tell the routines apart, and the call of
    /// that function is null, as an index finding its rows assumes (D135).
    fn by_values<V: Borrow<Value>>(&self, values: &[V]) -> Option<Chosen> {
        if self.fallback.is_some() && values.iter().all(|v| v.borrow().is_null()) {
            return None;
        }
        if let Some((last, given)) = values.split_last()
            && matches!(last.borrow(), Value::Array(_))
            && let Some(r) = self.routines.iter().find(|r| r.catalog && r.holds(given))
        {
            return Some((r, true));
        }
        self.routines.iter().find(|r| r.holds(values)).map(|r| (*r, false))
    }

    /// The value of a call of `values` and the completion condition it
    /// raises.
    ///
    /// # Errors
    ///
    /// The routine's; SQLCODE -103 if no routine takes the values.
    fn run(&self, values: &[Value], temp: Option<&dyn TempLobs>) -> Result<(Value, Option<SqlState>)> {
        let Some((routine, appended)) = self.by_values(values) else {
            return match &self.fallback {
                Some(f) => f.call_warned(values, temp),
                None => Err(self.refused()),
            };
        };
        let (given, read) = if appended { split(values) } else { (values, None) };
        routine.check_lengths(given)?;
        if routine.null_call && given.iter().any(Value::is_null) {
            return Ok((Value::Null, None));
        }
        let read = Read::of(read)?;
        let catalog = Catalog { units: &UNITS, lrms: &read.lrms, elements: &read.elements, metres_per_unit: 1.0_f64 };
        let args = Args { values: given, catalog, warning: Cell::new(None) };
        let v = (routine.run)(&args)?;
        Ok((v, args.warning.get()))
    }

    /// The routine a call whose arguments' types are those `known` has
    /// may invoke, none standing for a `<dynamic parameter
    /// specification>`; none if the function there was before takes it,
    /// its known arguments of no linear referencing type.
    fn by_known(&self, known: &[Option<DataType>]) -> Option<&'static Routine> {
        let lr = known.iter().flatten().any(|t| super::of_data_type(t).is_some());
        if self.fallback.is_some() && !lr {
            return None;
        }
        let fits = |r: &&&'static Routine| {
            r.params.len() == known.len() && r.params.iter().zip(known).all(|(p, t)| t.as_ref().is_none_or(|t| p.takes(t)))
        };
        self.routines.iter().find(fits).copied()
    }

    /// SQLCODE -103: no routine takes the arguments.
    fn refused(&self) -> SqlError {
        SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §9.18 SR10b] no routine {} takes the arguments given", self.name))
    }
}

/// The arguments of `values` and the catalog's ARRAY after them.
fn split(values: &[Value]) -> (&[Value], Option<&Value>) {
    match values.split_last() {
        Some((last, given)) => (given, Some(last)),
        None => (values, None),
    }
}

impl ScalarFunction for LrFunction {
    fn name(&self) -> &str {
        self.name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if let Some((r, _)) = self.by_types(args) {
            return Ok(r.result.data_type(args.first()));
        }
        match &self.fallback {
            Some(f) => f.return_type(args),
            None => Err(self.refused()),
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.run(args, None).map(|(v, _)| v)
    }

    fn call_with(&self, args: &[Value], temp: Option<&dyn TempLobs>) -> Result<Value> {
        match self.other(args) {
            Some(f) => f.call_with(args, temp),
            None => self.run(args, temp).map(|(v, _)| v),
        }
    }

    fn call_borrowed(&self, args: &[&Value], temp: Option<&dyn TempLobs>) -> Result<Value> {
        match self.other(args) {
            Some(f) => f.call_borrowed(args, temp),
            None => self.call_with(&args.iter().map(|v| (*v).clone()).collect::<Vec<_>>(), temp),
        }
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        match self.other(args) {
            Some(f) => f.call_in(args, cx),
            None => self.run(args, cx.temp_lobs()).map(|(v, _)| v),
        }
    }

    /// A batch of no call of a routine of §15 is the function's there was
    /// before; else a call a row.
    fn call_many(&self, rows: &[Vec<Value>]) -> Result<Vec<Value>> {
        match self.fallback.as_ref().filter(|_| rows.iter().all(|r| self.by_values(r).is_none())) {
            Some(f) => f.call_many(rows),
            None => rows.iter().map(|r| self.call(r)).collect(),
        }
    }

    fn call_warned(&self, args: &[Value], temp: Option<&dyn TempLobs>) -> Result<(Value, Option<SqlState>)> {
        self.run(args, temp)
    }

    /// ST_DefaultMeasure raises a warning [§15.2.4 Description 4], as may
    /// the function of the name there was before.
    fn warns(&self) -> bool {
        self.name == "ST_DEFAULTMEASURE" || self.fallback.as_ref().is_some_and(|f| f.warns())
    }

    /// Of a routine of §15, its parameter's type where a `<dynamic
    /// parameter specification>` stands, the declared type of an argument
    /// of SELF's; else that of the function there was before.
    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        match self.by_known(known) {
            Some(r) => {
                let own = known.first().cloned().flatten();
                let p = r.params.get(position).copied()?;
                known.get(position).cloned().flatten().is_none().then(|| p.data_type(own.as_ref()))
            }
            None => self.fallback.as_ref().and_then(|f| f.argument_type(position, known)),
        }
    }

    fn parameter_type(&self, position: usize, known: &[Option<DataType>]) -> Option<ParameterType> {
        match self.by_known(known) {
            Some(_) => self.argument_type(position, known).map(ParameterType::Of),
            None => self.fallback.as_ref().and_then(|f| f.parameter_type(position, known)),
        }
    }

    fn deterministic(&self) -> bool {
        self.fallback.as_ref().is_none_or(|f| f.deterministic())
    }

    /// Of a routine reading the catalogs, the query of the LRMs and the
    /// linear elements they hold (`sql/catalog.rs`).
    fn catalog_query(&self, constants: &[Option<&Value>], types: &[DataType], cat: &dyn crate::catalog::Catalog) -> Option<AppendedQuery> {
        match self.by_types(types) {
            Some((r, _)) => r.catalog.then(|| catalog::query(cat)),
            None => self.fallback.as_ref().and_then(|f| f.catalog_query(constants, types, cat)),
        }
    }

    /// Of a call of a routine of §15, none: a call is of one if an
    /// argument's type is one of §15's.
    fn index_safe(&self, args: &[KeyArgument<'_>]) -> bool {
        let lr = args.iter().filter_map(|a| a.data_type).any(|t| super::of_data_type(t).is_some());
        !lr && self.fallback.as_ref().is_some_and(|f| f.index_safe(args))
    }

    /// The function there was before, prepared of the constants `args`, and
    /// the routines of §15 still taking theirs; none if a constant is a
    /// value of §15's types, or the function there was before is not
    /// prepared.
    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        if args.iter().flatten().any(|v| super::kind::kind_of_value(v).is_some()) {
            return None;
        }
        let prepared = self.fallback.as_ref()?.specialize(args, budget)?;
        Some(ScalarRef(Arc::new(self.over(prepared.0))))
    }

    /// None: it is alone of its name, the overloads its routines and the
    /// function there was before (`argument_type`).
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        None
    }

    // The capabilities below are the function's there was before. An
    // index, a plan or a batch reads them of a call whose column or probe
    // is a geometry, an argument no routine of §15 takes, so a call of a
    // routine of §15 never reaches what they promise.

    fn configurable(&self) -> Option<&dyn Configurable> {
        self.fallback.as_ref().and_then(|f| f.configurable())
    }

    fn index_predicate(&self) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.index_predicate())
    }

    fn predicate_truth(&self) -> Option<Value> {
        self.fallback.as_ref().and_then(|f| f.predicate_truth())
    }

    fn rechecked_errors(&self) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.rechecked_errors())
    }

    fn probe_arguments(&self) -> Option<usize> {
        self.fallback.as_ref().and_then(|f| f.probe_arguments())
    }

    fn json_probe(&self) -> Option<JsonProbe<'_>> {
        self.fallback.as_ref().and_then(|f| f.json_probe())
    }

    fn appended_query(&self, constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        self.fallback.as_ref().and_then(|f| f.appended_query(constants))
    }

    fn ranked_by_index(&self) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.ranked_by_index())
    }

    fn ranks(&self) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.ranks())
    }

    fn uses_context(&self, arity: usize) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.uses_context(arity))
    }

    fn spatial_operation(&self) -> Option<SpatialOperation> {
        self.fallback.as_ref().and_then(|f| f.spatial_operation())
    }

    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        self.fallback.as_ref().and_then(|f| f.vector_metric(constants))
    }

    fn word_argument(&self, position: usize, word: &str, first: bool) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.word_argument(position, word, first))
    }

    fn batches(&self) -> bool {
        self.fallback.as_ref().is_some_and(|f| f.batches())
    }
}
