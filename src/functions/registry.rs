//! The functions a database knows, by name (RULES.md D53, D74): scalar
//! functions, each alone of its name or one of overloads told apart by
//! the types of their parameters, and aggregate functions, the standard
//! set functions among them.

use std::collections::HashMap;
use std::sync::Arc;

use super::{AggregateFunction, AggregateRef, Avg, Count, Extreme, ScalarFunction, ScalarRef, Sum, Truths, any_value, strings, trim};
use crate::error::{Code, Result, fail};
use crate::fulltext::matches::TextMatches;
use crate::value::DataType;
use crate::value::precedence::told_apart;

/// The functions a database knows, by name: scalar and aggregate names never
/// overlap, and the scalar functions of a name are those of their
/// registration, in its order.
#[derive(Clone)]
pub struct Registry {
    scalars: HashMap<String, Vec<ScalarRef>>,
    aggregates: HashMap<String, AggregateRef>,
    /// The standard COUNT, MAX, MIN, SUM and AVG, in `Standard`'s order,
    /// which a plug-in of its name may replace.
    standard: [AggregateRef; 5],
}

/// A set function a `Registry` holds as the standard defines it, no
/// plug-in of its name [ISO/IEC 9075:1992 §6.5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standard {
    /// COUNT.
    Count,
    /// MAX.
    Max,
    /// MIN.
    Min,
    /// SUM.
    Sum,
    /// AVG.
    Avg,
}

/// The built-in functions of full text (`fulltext/matches.rs`).
fn words() -> [Arc<dyn ScalarFunction>; 1] {
    [Arc::new(TextMatches)]
}

impl Registry {
    /// A registry holding the five SQL-86 set functions, EVERY, ANY and
    /// SOME [ISO/IEC 9075-2:2023 §10.9], the string functions of
    /// SQL-92 [ISO/IEC 9075:1992 §6.6, §6.7, §6.13], and TEXT_MATCHES
    /// \[EXTENSION\], which a full-text index answers (RULES.md D86).
    pub fn with_set_functions() -> Registry {
        let standard = [
            AggregateRef(Arc::new(Count)),
            AggregateRef(Arc::new(Extreme { max: true })),
            AggregateRef(Arc::new(Extreme { max: false })),
            AggregateRef(Arc::new(Sum)),
            AggregateRef(Arc::new(Avg)),
        ];
        let scalars = strings::functions()
            .into_iter()
            .chain(trim::functions())
            .chain(words())
            .map(|f| (f.name().to_string(), vec![ScalarRef(f)]))
            .collect();
        let truths = [Truths::EVERY, Truths::ANY, Truths::SOME].map(|t| AggregateRef(Arc::new(t)));
        let any_value = [AggregateRef(Arc::new(any_value::AnyValue))];
        let aggregates = standard.iter().chain(&truths).chain(&any_value).map(|f| (f.0.name().to_string(), f.clone())).collect();
        Registry { scalars, aggregates, standard }
    }

    /// Adds a scalar function: of a name already a scalar function's, a
    /// standard one's too, one more only if each declares the types of its
    /// parameters and no call could invoke two of them alike [ISO/IEC
    /// 9075-2:2023 §11.60 SR20b] (RULES.md D53, D74).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is an aggregate function's (a call could not
    /// tell which is meant), or a scalar function's and not so told apart.
    pub fn add_scalar(&mut self, f: Arc<dyn ScalarFunction>) -> Result<()> {
        let name = f.name().to_string();
        if self.aggregates.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already an aggregate function; a name cannot be both"));
        }
        if let Some(others) = self.scalars.get(&name) {
            let theirs: Option<Vec<Vec<DataType>>> = others.iter().map(|o| o.0.parameter_types()).collect();
            let (Some(mine), Some(theirs)) = (f.parameter_types(), theirs) else {
                return fail(
                    Code::Api,
                    format!(
                        "[EXTENSION] {name} is already a scalar function, and is not replaced, as one of them declares no parameter types"
                    ),
                );
            };
            if !told_apart(&mine, &theirs.iter().map(Vec::as_slice).collect::<Vec<_>>()) {
                return fail(
                    Code::Api,
                    format!("[EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b] {name} has a function no call could tell from this one"),
                );
            }
        }
        self.scalars.entry(name).or_default().push(ScalarRef(f));
        Ok(())
    }

    /// Adds or replaces an aggregate function, including the five set functions.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is a scalar function's.
    pub fn add_aggregate(&mut self, f: Arc<dyn AggregateFunction>) -> Result<()> {
        let name = f.name().to_string();
        if self.scalars.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already a scalar function; a name cannot be both"));
        }
        self.aggregates.insert(name, AggregateRef(f));
        Ok(())
    }

    /// Each function registered that declares settings (RULES.md D36).
    pub(crate) fn configurables(&self) -> impl Iterator<Item = &dyn crate::settings::Configurable> {
        let scalars = self.scalars.values().flatten().filter_map(|f| f.0.configurable());
        scalars.chain(self.aggregates.values().filter_map(|f| f.0.configurable()))
    }

    /// The scalar function named `name` registered first, if one is.
    pub fn scalar(&self, name: &str) -> Option<ScalarRef> {
        self.scalars.get(name).and_then(|fs| fs.first()).cloned()
    }

    /// The scalar functions named `name`, in the order registered: more
    /// than one if overloaded (RULES.md D74).
    pub fn scalars(&self, name: &str) -> Vec<ScalarRef> {
        self.scalars.get(name).cloned().unwrap_or_default()
    }

    /// The aggregate function named `name`, if one is registered.
    pub fn aggregate(&self, name: &str) -> Option<AggregateRef> {
        self.aggregates.get(name).cloned()
    }

    /// Which of the standard COUNT, MAX, MIN, SUM and AVG [X3.135-1986
    /// §5.8] `f` is, if it is one, no plug-in of its name.
    pub(crate) fn standard(&self, f: &AggregateRef) -> Option<Standard> {
        let held = |at: usize| self.standard.get(at).is_some_and(|s| std::ptr::addr_eq(Arc::as_ptr(&f.0), Arc::as_ptr(&s.0)));
        [Standard::Count, Standard::Max, Standard::Min, Standard::Sum, Standard::Avg]
            .into_iter()
            .enumerate()
            .find_map(|(at, s)| held(at).then_some(s))
    }
}
