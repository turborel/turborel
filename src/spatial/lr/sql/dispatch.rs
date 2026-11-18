//! The routines of §15 of one name as one function: as it is bound, the
//! routine whose parameters the declared types of its arguments fit; as
//! it runs, the routine whose parameters its values fit, the null value
//! fitting any. A function of the name already there, a geometry
//! routine, is invoked of arguments no routine of §15 takes.

use std::cell::Cell;
use std::sync::{Arc, LazyLock};

use super::super::{Catalog, Unit, linear_units};
use super::args::Args;
use super::catalog::{self, Read};
use super::routine::Routine;
use super::routines;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::params::ParameterType;
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

    /// The routine the declared types `types` choose.
    fn by_types(&self, types: &[DataType]) -> Option<Chosen> {
        let own = self.routines.iter().find(|r| r.takes(types)).map(|r| (*r, false));
        own.or_else(|| {
            let (_, given) = types.split_last()?;
            self.routines.iter().find(|r| r.catalog && r.takes(given)).map(|r| (*r, true))
        })
    }

    /// The routine the values `values` choose.
    fn by_values(&self, values: &[Value]) -> Option<Chosen> {
        if let Some((last, given)) = values.split_last()
            && matches!(last, Value::Array(_))
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
    fn run(&self, values: &[Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<(Value, Option<SqlState>)> {
        let Some((routine, appended)) = self.by_values(values) else {
            return match &self.fallback {
                Some(f) => f.call_warned(values, temp),
                None => Err(self.refused()),
            };
        };
        let (given, read) = if appended { split(values) } else { (values, None) };
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

    fn call_warned(&self, args: &[Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<(Value, Option<SqlState>)> {
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
}
