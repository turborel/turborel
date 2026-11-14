//! [ISO/IEC 9075-2:2023 §4.19 to §4.23, §6.27, §7.7 to §7.9, §9.41] Row
//! pattern recognition bound.
//!
//! The `<row pattern measures>` and `<row pattern definition search
//! condition>`s are bound by a binder of their own (`Recognizing`), over a
//! row of the row pattern input table's columns once for each row pattern
//! variable, the universal one last and unnamed [§7.9 SR18]. So `A.PRICE`
//! is column `PRICE` of variable `A`'s copy and `PRICE` of the universal
//! one's, which every lookup finds as it finds any column. A reference
//! found so is placed by its scope (`Place`). In a measure or definition
//! it is a value of its own, `RUNNING LAST (A.PRICE)` [§6.7 SR7b i 4], one
//! of the `PatternValue`s it is an expression over. In a row pattern
//! navigation operation or a set function's aggregated argument it is the
//! column of a row of the input, the one variable those read being noted
//! [§6.27 SR4, §10.9 SR15]. In an ordered set function's argument not
//! aggregated, it is a column of a row of the input functionally dependent
//! on the row pattern partitioning columns [§6.7 SR9].

mod common;
mod table;
mod window;

use std::cell::{Cell, RefCell};

use super::dependencies::Known;
use super::{Binder, Grouping, Scope, rule};
use crate::ast::{
    Literal, Logical, Navigation, Physical, Semantics, SetArg, SetFunctionSpecification, ValueExpression, ValueSpecification,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, Navigated, PatternAggregate, PatternValue};
use crate::value::{DataType, Decimal, Value};

pub(super) use table::recognized;
pub(super) use window::{Matched, placed, window_measure};

/// [ISO/IEC 9075-2:2023 §6.27 SR2] The rule a set function, a window
/// function or a navigation breaks in a navigation's value expression.
const NAVIGATED: &str = "[ISO/IEC 9075-2:2023 §6.27 SR2] the value expression of a <row pattern navigation operation> contains no set function, window function or <row pattern navigation operation>";

/// [ISO/IEC 9075-2:2023 §6.27 SR2, §6.9 SR13] The rule a navigation in a
/// navigation or in a set function's aggregated argument breaks.
const NESTED: &str = "[ISO/IEC 9075-2:2023 §6.27 SR2, §6.9 SR13] a <row pattern navigation operation> is in no <row pattern navigation operation> and no aggregated argument of a set function";

/// [ISO/IEC 9075-2:2023 §6.27 Format] What an offset of a navigation is.
const OFFSET: &str = "[ISO/IEC 9075-2:2023 §6.27] a <logical offset> or <physical offset> is a <simple value specification> or a <dynamic parameter specification>";

/// What binds row pattern column references, the universal row pattern
/// variable's and each named variable's, and the values a measure or a
/// definition reads of a match.
pub(super) struct Recognizing {
    /// The row pattern variables' names: the primary ones, then the union
    /// ones. The universal one is numbered after them.
    names: Vec<String>,
    /// The degree of the row pattern input table: how many columns each
    /// variable has in the row the binder binds over.
    width: usize,
    /// Of a row of the input as a value reads it, the column of its
    /// classifier; the match number's is the next.
    classifier: usize,
    /// Whether a `<row pattern definition search condition>` is bound,
    /// in which FINAL is not [§6.27 SR5b, §6.9 SR12].
    define: Cell<bool>,
    /// Of a `<row pattern recognition clause>`, not a window, which alone
    /// has MATCH_NUMBER [§6.31 SR22a].
    table: bool,
    /// The row pattern partitioning columns, or window partitioning
    /// columns, of the input [§6.7 SR9].
    partition: Vec<usize>,
    /// The functional dependencies the input is known to have.
    known: Known,
    /// A measure's or definition's scope, its set functions its values.
    measure: Grouping,
    /// The scope of an ordered set function's arguments not aggregated.
    invariant: Grouping,
    /// The values read so far.
    slots: RefCell<Vec<PatternValue>>,
    /// The one row pattern variable a navigation or an aggregated argument
    /// read so far, if any.
    found: Cell<Option<usize>>,
    /// Whether a navigation, not a set function, is bound.
    navigating: Cell<bool>,
    /// RUNNING or FINAL of the set function bound.
    semantics: Cell<Option<Semantics>>,
}

/// The shape of a `Recognizing`: its variables, the input's degree and its
/// width as a value reads a row of it, whether it is a table's, and the
/// partitioning columns and dependencies of the input.
pub(super) struct Shape {
    pub(super) names: Vec<String>,
    pub(super) width: usize,
    pub(super) classifier: usize,
    pub(super) table: bool,
    pub(super) partition: Vec<usize>,
    pub(super) known: Known,
}

/// Where a row pattern column reference, a classifier or a set function
/// is bound.
#[derive(Clone, Copy)]
enum Place {
    /// In a measure or a definition, outside a navigation and an
    /// aggregated argument.
    Measure,
    /// In a navigation's value expression or an aggregated argument.
    Within,
    /// In an ordered set function's argument not aggregated.
    Invariant,
}

impl Recognizing {
    /// One of `shape`, no value read yet.
    pub(super) fn new(shape: Shape) -> Recognizing {
        let Shape { names, width, classifier, table, partition, known } = shape;
        Recognizing {
            names,
            width,
            classifier,
            define: Cell::new(false),
            table,
            partition,
            known,
            measure: Grouping::empty(),
            invariant: Grouping::empty(),
            slots: RefCell::new(Vec::new()),
            found: Cell::new(None),
            navigating: Cell::new(false),
            semantics: Cell::new(None),
        }
    }

    /// The universal row pattern variable's number.
    fn universal(&self) -> usize {
        self.names.len()
    }

    /// The row pattern variable named `name` [§7.9 SR4, SR5].
    fn variable(&self, name: &str) -> Result<usize> {
        self.names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §7.9 SR5] no row pattern variable {name} is in scope")))
    }

    /// The row pattern variable and column of the input of position `pos`
    /// of the row bound over.
    fn decoded(&self, pos: usize) -> Result<(usize, usize)> {
        let design = || SqlError::new(Code::Api, "[DESIGN] a row pattern column reference is of no variable");
        let variable = pos.checked_div(self.width).ok_or_else(design)?;
        let column = pos.checked_rem(self.width).ok_or_else(design)?;
        Ok((variable, column))
    }

    /// Where `scope` is.
    fn place(&self, scope: Scope<'_>) -> Result<Place> {
        let design = || SqlError::new(Code::Api, "[DESIGN] a row pattern is bound in a scope of its own");
        match scope {
            Scope::Groups(g, _) => {
                if std::ptr::eq(g, std::ptr::from_ref(&self.measure)) {
                    Ok(Place::Measure)
                } else if std::ptr::eq(g, std::ptr::from_ref(&self.invariant)) {
                    Ok(Place::Invariant)
                } else {
                    Err(design())
                }
            }
            Scope::Rows(_) => Ok(Place::Within),
            Scope::Select => Err(design()),
        }
    }

    /// The scope of a measure or definition, set functions its values.
    pub(super) fn scope(&self) -> Scope<'_> {
        Scope::Groups(&self.measure, "[ISO/IEC 9075-2:2023 §7.8] a <row pattern measure expression> is bound over the rows of a match")
    }

    /// `value` read, a column of the row of the values: its number.
    fn slot(&self, value: PatternValue) -> usize {
        let mut slots = self.slots.borrow_mut();
        slots.push(value);
        slots.len().saturating_sub(1)
    }

    /// The values read so far, none read after.
    pub(super) fn take_slots(&self) -> Vec<PatternValue> {
        self.slots.take()
    }

    /// Sets whether a definition, in which FINAL is not, is bound.
    pub(super) fn defining(&self, define: bool) {
        self.define.set(define);
    }

    /// [§6.27 SR4, §10.9 SR15] Notes that the navigation or aggregated
    /// argument bound reads `variable`, the one it reads.
    fn found(&self, variable: usize) -> Result<()> {
        if self.found.get().is_some_and(|v| v != variable) {
            return Err(rule(if self.navigating.get() {
                "[ISO/IEC 9075-2:2023 §6.27 SR4] one row pattern variable qualifies every column reference of a <row pattern navigation operation> and is the argument of each of its CLASSIFIERs"
            } else {
                "[ISO/IEC 9075-2:2023 §10.9 SR15] one row pattern variable qualifies every column reference of a set function's aggregated argument and is the argument of each of its CLASSIFIERs"
            }));
        }
        self.found.set(Some(variable));
        Ok(())
    }

    /// [§6.7 SR9] That column `column` of the input, referenced in an
    /// ordered set function's argument not aggregated, is functionally
    /// dependent on the partitioning columns.
    fn invariant_column(&self, column: usize) -> Result<()> {
        if self.partition.contains(&column) || self.known.determines(&self.partition, column) {
            return Ok(());
        }
        Err(rule(
            "[ISO/IEC 9075-2:2023 §6.7 SR9] a column referenced in a set function's argument not aggregated is functionally dependent on the partitioning columns",
        ))
    }

    /// `expr` of `variable`'s current row: RUNNING LAST (`expr`) [§6.27
    /// SR8].
    fn running_last(&self, expr: Expr, variable: usize) -> usize {
        let zero = Expr::Const(Value::Exact(Decimal::new(0, 0)));
        let navigated = Navigated { expr, variable, final_: false, last: true, logical: zero.clone(), next: false, physical: zero };
        self.slot(PatternValue::Navigated(Box::new(navigated)))
    }
}

impl Grouping {
    /// A grouping of no grouping columns and no set function yet.
    fn empty() -> Grouping {
        Grouping {
            columns: Vec::new(),
            collations: Vec::new(),
            aggs: RefCell::new(Vec::new()),
            collated: RefCell::new(Vec::new()),
            sets: None,
            determinants: Vec::new(),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.33 SR24b] The type of CLASSIFIER: of an
/// SQL_IDENTIFIER, CHARACTER VARYING of the longest identifier (IL048).
fn classifier_type() -> DataType {
    crate::system::identifier("", false).data_type
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.7 SR7b i, SR9] Of a row pattern binder, the
    /// column at `pos` of the row it binds over, in `scope`; none of
    /// another binder.
    pub(super) fn pattern_column(&self, pos: usize, scope: Scope<'_>) -> Result<Option<Expr>> {
        let Some(r) = self.recognizing.as_deref() else { return Ok(None) };
        let (variable, column) = r.decoded(pos)?;
        match r.place(scope)? {
            Place::Measure => Ok(Some(Expr::Column(r.running_last(Expr::Column(column), variable)))),
            Place::Within => {
                r.found(variable)?;
                Ok(Some(Expr::Column(column)))
            }
            Place::Invariant => {
                r.invariant_column(column)?;
                Ok(Some(Expr::Column(column)))
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.27] A `<row pattern navigation operation>`,
    /// its value expression of one row pattern variable [SR3, SR4], its
    /// offsets of exact numeric types of scale 0, not negative literals
    /// [SR5c, SR6a], RUNNING and offset 0 of FIRST or LAST and offset 1 of
    /// PREV or NEXT implicit [SR5, SR6, SR7, SR8].
    pub(super) fn navigation(&self, n: &Navigation, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let Some(r) = self.recognizing.as_deref() else {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.27 SR1] a <row pattern navigation operation> is contained in a <row pattern measures> or a <row pattern definition search condition>").or_at(n.at));
        };
        match r.place(scope)? {
            Place::Measure => {}
            Place::Within => return Err(rule(NESTED).or_at(n.at)),
            Place::Invariant => return Err(not_invariant("a <row pattern navigation operation>").or_at(n.at)),
        }
        let (final_, last, logical) = match &n.logical {
            Some(Logical { final_, last, offset }) => (*final_, *last, offset.as_deref()),
            None => (false, true, None),
        };
        if final_ && r.define.get() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.27 SR5b] FINAL is not in a <row pattern definition search condition>").or_at(n.at));
        }
        r.navigating.set(true);
        let bound = self.value_expression(&n.expr, Scope::Rows(NAVIGATED));
        r.navigating.set(false);
        let found = r.found.take();
        let (expr, t) = bound?;
        let variable = found.ok_or_else(|| {
            rule("[ISO/IEC 9075-2:2023 §6.27 SR3] the value expression of a <row pattern navigation operation> contains a row pattern column reference or CLASSIFIER").or_at(n.at)
        })?;
        let logical = self.offset(logical, 0)?;
        let (next, physical) = match &n.physical {
            Some(Physical { next, offset }) => (*next, self.offset(offset.as_deref(), 1)?),
            None => (false, Expr::Const(Value::Exact(Decimal::new(0, 0)))),
        };
        let navigated = Navigated { expr, variable, final_, last, logical, next, physical };
        Ok((Expr::Column(r.slot(PatternValue::Navigated(Box::new(navigated)))), t))
    }

    /// [ISO/IEC 9075-2:2023 §6.27 SR5c, SR6a] The offset `written` of a
    /// navigation, `implied` if none is: of a `<simple value
    /// specification>` or a `?` INTEGER, exact numeric of scale 0, no
    /// negative literal.
    fn offset(&self, written: Option<&ValueExpression>, implied: i128) -> Result<Expr> {
        let Some(v) = written else { return Ok(Expr::Const(Value::Exact(Decimal::new(implied, 0)))) };
        let ValueExpression::Value(spec) = v else {
            // Deactivated: the parser reads an offset as a `<simple value
            // specification>` or a `?` alone (`number_of_rows`).
            return Err(rule(OFFSET));
        };
        // [X3.135-1986 §5.2, §5.3 SR1] A sign written against its number is
        // of the literal, so -0 is zero, and "- 1" is no literal.
        let negative = if let ValueSpecification::Literal(Literal::Exact(d)) = spec { d.units < 0 } else { false };
        if negative {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §6.27 SR5c i, SR6a] a literal offset of a <row pattern navigation operation> is not negative",
            ));
        }
        let (e, t) = self.typed(v, crate::params::ParameterType::Of(super::composite::integer()), Scope::Rows(OFFSET))?;
        if !matches!(t, DataType::Exact { scale: 0, .. }) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §6.27 SR5c i, SR6a] an offset of a <row pattern navigation operation> is exact numeric of scale 0, not {t}"
            )));
        }
        Ok(e)
    }

    /// [ISO/IEC 9075-2:2023 §6.33] `<classifier function>` of `variable`,
    /// the universal row pattern variable if none [SR24c]: RUNNING LAST
    /// (CLASSIFIER (`variable`)) outside a navigation and an aggregated
    /// argument [SR24d].
    pub(super) fn classifier(&self, variable: Option<&str>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let Some(r) = self.recognizing.as_deref() else {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §6.33 SR24a] CLASSIFIER is contained in a <row pattern measures> or a <row pattern definition search condition>",
            ));
        };
        let v = match variable {
            Some(name) => r.variable(name)?,
            None => r.universal(),
        };
        let column = Expr::Column(r.classifier);
        match r.place(scope)? {
            Place::Measure => Ok((Expr::Column(r.running_last(column, v)), classifier_type())),
            Place::Within => {
                r.found(v)?;
                Ok((column, classifier_type()))
            }
            Place::Invariant => Err(not_invariant("CLASSIFIER")),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.31] `<match number function>`, of a `<row
    /// pattern recognition clause>` alone [SR22a], exact numeric of scale
    /// 0, COUNT's type [SR22b].
    pub(super) fn match_number(&self, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let Some(r) = self.recognizing.as_deref().filter(|r| r.table) else {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.31 SR22a] MATCH_NUMBER is contained in a <row pattern recognition clause>"));
        };
        let t = self.count_type()?;
        match r.place(scope)? {
            Place::Measure => Ok((Expr::Column(r.slot(PatternValue::MatchNumber)), t)),
            Place::Within | Place::Invariant => Ok((Expr::Column(r.classifier.saturating_add(1)), t)),
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.9 SR6] `<row pattern count function>`
    /// COUNT (`name`.*): COUNT of the rows mapped to the row pattern
    /// variable `name`.
    pub(super) fn pattern_count(&self, sf: &SetFunctionSpecification, name: &str, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let Some(r) = self.recognizing.as_deref() else {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §10.9 SR6] a <row pattern count function> is contained in a <row pattern measures> or a <row pattern definition search condition>",
            ));
        };
        let variable = r.variable(name)?;
        if let Place::Measure | Place::Invariant = r.place(scope)? {
            r.found(variable)?;
        }
        let count = self.db.functions.aggregate(kw::COUNT).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] COUNT is not registered"))?;
        self.aggregate(count, (None, false), sf.filter.as_deref(), scope)
    }

    /// [ISO/IEC 9075-2:2023 §6.9 SR11, SR12] Of the set function `sf` in
    /// `scope`: RUNNING or FINAL in row pattern matching alone, no FINAL
    /// in a definition, no grouping operation there; and the scope of an
    /// ordered set function's arguments not aggregated.
    pub(super) fn pattern_set_function<'s>(&'s self, sf: &SetFunctionSpecification, scope: Scope<'s>) -> Result<Scope<'s>> {
        let Some(r) = self.recognizing.as_deref() else {
            if sf.semantics.is_some() && !self.in_pattern() {
                return Err(rule(
                    "[ISO/IEC 9075-2:2023 §6.9 SR11b] RUNNING or FINAL is written before a set function in row pattern matching alone",
                ));
            }
            return Ok(scope);
        };
        if matches!(sf.arg, SetArg::Grouping(_)) {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §6.9 SR11a i] a <grouping operation> is not in a <row pattern measures> or a <row pattern definition search condition>",
            ));
        }
        if sf.semantics == Some(Semantics::Final) && r.define.get() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.9 SR12] FINAL is not in a <row pattern definition search condition>"));
        }
        r.semantics.set(sf.semantics);
        let ordered = matches!(sf.arg, SetArg::Hypothetical(_) | SetArg::Fraction(_));
        match (r.place(scope)?, ordered) {
            (Place::Measure, true) => {
                Ok(Scope::Groups(&r.invariant, "[ISO/IEC 9075-2:2023 §6.7 SR9] an argument not aggregated is of the match"))
            }
            (Place::Measure | Place::Within | Place::Invariant, _) => Ok(scope),
        }
    }

    /// Whether it binds in row pattern matching: it, or a query it is a
    /// subquery of.
    fn in_pattern(&self) -> bool {
        let mut binder = self;
        loop {
            if binder.recognizing.is_some() {
                return true;
            }
            match binder.outer {
                Some(outer) => binder = outer.binder(),
                None => return false,
            }
        }
    }

    /// The set function `call` of the groups `g`: of row pattern matching,
    /// an aggregate over the rows mapped to the one row pattern variable
    /// its aggregated argument reads, the universal one if none [§10.9
    /// GR1a to c, SR5].
    pub(super) fn registered_in(
        &self,
        g: &Grouping,
        call: AggCall,
        collation: Option<crate::value::collation::CollationRef>,
    ) -> (Expr, DataType) {
        // An ordered set function of a measure is bound in the invariant
        // groups, which no other set function is in [§10.9 SR12d, SR14].
        let ours = |r: &&Recognizing| std::ptr::eq(g, std::ptr::from_ref(&r.measure)) || std::ptr::eq(g, std::ptr::from_ref(&r.invariant));
        let Some(r) = self.recognizing.as_deref().filter(ours) else {
            return super::set_functions::registered(g, call, collation);
        };
        let variable = r.found.take().unwrap_or_else(|| r.universal());
        let final_ = r.semantics.take() == Some(Semantics::Final);
        let result = call.result.clone();
        let slot = r.slot(PatternValue::Aggregate(Box::new(PatternAggregate { call, variable, final_ })));
        (Expr::Column(slot), result)
    }

    /// The scope of an ordered set function's argument not aggregated:
    /// `scope`, of row pattern matching, which `pattern_set_function` made
    /// the invariant groups' where it may be, else `otherwise`.
    pub(super) fn non_aggregated<'s>(&self, scope: Scope<'s>, otherwise: Scope<'s>) -> Scope<'s> {
        if self.recognizing.is_some() { scope } else { otherwise }
    }
}

/// \[DESIGN\] `what` in an ordered set function's argument not aggregated,
/// of row pattern matching: not supported.
fn not_invariant(what: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] {what} in an argument of an ordered set function not aggregated is not supported"))
}
