//! \[EXTENSION\] VECTOR in expressions (design document §6, §8; RULES.md
//! D88): the key words a function of it takes as arguments, given as
//! character strings (`VECTOR_DISTANCE(a, b, COSINE)`); `+`, `-`, `*` and
//! `/` of vectors, calls of VECTOR_ADD and its kin; and VECTOR_DISTANCE by
//! a distance function of SQL (CREATE VECTOR DISTANCE FUNCTION), whose
//! body is bound here, over its two parameters, into a metric.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use super::views::Source;
use super::{Binder, Context, FromItem, Purpose, Scope, rule};
use crate::ast::{ColumnSpecification, Literal, ValueExpression, ValueSpecification};
use crate::catalog::{ColumnDesc, QualifiedName, SqlRoutineDesc, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::exec::{NoSubqueries, eval};
use crate::functions::ScalarRef;
use crate::logical::Expr;
use crate::params::Params;
use crate::parser::Parser;
use crate::value::{ArithOp, DataType};
use crate::vector::functions::VECTOR_DISTANCE;
use crate::vector::metric::{CallbackMetric, DistanceFn, MetricProperties, VectorMetric, metrics_with};
use crate::vector::{Element, Vector, data_type, functions_with, shape_of};

/// A character string literal of `text`.
fn character(text: String) -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(Literal::Character(text)))
}

/// The word an identifier written alone as an argument is: `w`, or `s.w`.
fn word_of(arg: &ValueExpression) -> Option<String> {
    let ValueExpression::Column(ColumnSpecification { qualifier, column, catalog: None, .. }) = arg else { return None };
    let Some(q) = qualifier else { return Some(column.clone()) };
    q.auth.is_none().then(|| format!("{}.{column}", q.ident))
}

/// The text of a character string literal.
fn literal_text(arg: Option<&ValueExpression>) -> Option<&str> {
    match arg {
        Some(ValueExpression::Value(ValueSpecification::Literal(Literal::Character(text)))) => Some(text),
        _ => None,
    }
}

/// The function of VECTOR whose operator `op` is between operands of
/// `left` and `right`, the first a vector, and whether its operands are
/// swapped: a number times a vector scales it.
fn operator_of(op: ArithOp, left: &DataType, right: &DataType) -> Option<(&'static str, bool)> {
    let vector = |t: &DataType| shape_of(t).is_some() || t.is_character();
    match (op, vector(left), vector(right)) {
        (ArithOp::Add, true, true) => Some(("VECTOR_ADD", false)),
        (ArithOp::Sub, true, true) => Some(("VECTOR_SUBTRACT", false)),
        (ArithOp::Mul, true, true) => Some(("VECTOR_MULTIPLY", false)),
        (ArithOp::Mul, true, false) => Some(("VECTOR_SCALE", false)),
        (ArithOp::Mul, false, true) => Some(("VECTOR_SCALE", true)),
        (ArithOp::Div, true, false) => Some(("VECTOR_DIVIDE", false)),
        (ArithOp::Add | ArithOp::Sub | ArithOp::Div, _, _) | (ArithOp::Mul, false, false) => None,
    }
}

impl Binder<'_> {
    /// The arguments `args` of a call of the registered function `name`,
    /// each identifier written alone that a function of the name takes as
    /// a key word there (`ScalarFunction::word_argument`) made a character
    /// string of it.
    pub(super) fn worded<'a>(&self, name: &str, args: &'a [ValueExpression]) -> Cow<'a, [ValueExpression]> {
        let registered = self.db.functions.scalars(name);
        if registered.is_empty() {
            return Cow::Borrowed(args);
        }
        let mut first = literal_text(args.first()).is_some();
        let mut worded: Option<Vec<ValueExpression>> = None;
        for (at, arg) in args.iter().enumerate() {
            let Some(word) = word_of(arg) else { continue };
            if !registered.iter().any(|f| f.0.word_argument(at, &word, first)) {
                continue;
            }
            first |= at == 0;
            if let Some(slot) = worded.get_or_insert_with(|| args.to_vec()).get_mut(at) {
                *slot = character(word);
            }
        }
        worded.map_or(Cow::Borrowed(args), Cow::Owned)
    }

    /// `+`, `-`, `*` or `/` of operands bound, if one is a vector: the call
    /// of the function of VECTOR it stands for (design document §6.5), a
    /// character string operand cast to VECTOR. None if neither is one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for an operator vectors do not take, as `/` of two.
    pub(super) fn vector_arithmetic(
        &self,
        left: &(Expr, DataType),
        op: ArithOp,
        right: &(Expr, DataType),
    ) -> Option<Result<(Expr, DataType)>> {
        if shape_of(&left.1).is_none() && shape_of(&right.1).is_none() {
            return None;
        }
        let Some((name, swapped)) = operator_of(op, &left.1, &right.1) else {
            return Some(Err(rule(format!("[EXTENSION] {:?} of {} and {} is no operation of vectors", op, left.1, right.1))));
        };
        let (first, second) = if swapped { (right, left) } else { (left, right) };
        let made = |(e, t): &(Expr, DataType), number: bool| -> (Expr, DataType) {
            if number || !t.is_character() { (e.clone(), t.clone()) } else { (Expr::Cast(Box::new(e.clone()), data_type()), data_type()) }
        };
        let scalar = matches!(name, "VECTOR_SCALE" | "VECTOR_DIVIDE");
        let (one, other) = (made(first, false), made(second, scalar));
        let function = self.db.functions.scalar(name)?;
        Some(function.0.return_type(&[one.1, other.1]).map(|t| (Expr::Call(function, vec![one.0, other.0]), t)))
    }

    /// The arguments `args` of VECTOR_DISTANCE written with no metric, its
    /// first a column of a base table: with the metric an index on that
    /// column, its first, describes (`IndexMethod::metric`), if one does
    /// (design document §6.1 R6.3). Else COSINE, VECTOR_DISTANCE's own
    /// default, measures them, and None.
    pub(super) fn index_metric(&self, name: &str, args: &[ValueExpression]) -> Option<Vec<ValueExpression>> {
        let [ValueExpression::Column(cs), _] = args else { return None };
        if name != VECTOR_DISTANCE {
            return None;
        }
        let (item, i, _) = match &cs.qualifier {
            Some(q) => self.qualified(q, &cs.column).ok()?,
            None => self.unqualified(&cs.column).ok()?,
        };
        if !matches!(item.source, Source::Table) {
            return None;
        }
        let indexes = self.db.catalog.indexes(&item.table);
        let mut metrics = indexes.iter().filter(|ix| ix.columns.first() == Some(&i));
        let metric = metrics.find_map(|ix| self.db.methods.get(&ix.method).and_then(|m| m.metric(ix)))?;
        Some([args, &[character(metric)]].concat())
    }

    /// VECTOR_DISTANCE `args`, if its metric is named by a character string
    /// that names a distance function of SQL: the call measuring by it
    /// (design document §8). None if not, and VECTOR_DISTANCE is bound as
    /// any registered function.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the user holds no EXECUTE on it (R8.5); as its body
    /// binds; as the arguments bind.
    pub(super) fn distance_call(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Option<Result<(Expr, DataType)>> {
        if name != VECTOR_DISTANCE {
            return None;
        }
        let at = if literal_text(args.first()).is_some() { 0 } else { 2 };
        let desc = distance_routine(self.db, literal_text(args.get(at))?, self.schema)?;
        Some(self.measured_by(desc, args, at, scope))
    }

    /// VECTOR_DISTANCE `args`, its metric, argument `at`, the distance
    /// function `desc`.
    fn measured_by(&self, desc: &SqlRoutineDesc, args: &[ValueExpression], at: usize, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if !desc.execute_of(self.who()).0 {
            return Err(SqlError::new(Code::Privilege, format!("[EXTENSION] {} has no EXECUTE privilege on {}", self.auth, desc.name)));
        }
        // Noted, so a definition that names it depends on it [ISO/IEC
        // 9075-2:2023 §4.35.2], as one that invokes a function does.
        super::invocation::note_specific(self.invoked, &desc.specific);
        let metric = sql_metric(self.db, desc)?;
        let mut named = args.to_vec();
        if let Some(slot) = named.get_mut(at) {
            *slot = character(metric.name().to_owned());
        }
        let measuring = functions_with(&metrics_with(vec![metric]));
        let f = measuring
            .into_iter()
            .find(|f| f.name() == VECTOR_DISTANCE)
            .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] no VECTOR_DISTANCE"))?;
        let (exprs, types) = self.arguments(&*f, &named, scope)?;
        let t = f.return_type(&types)?;
        Ok((Expr::Call(ScalarRef(f), exprs), t))
    }
}

/// The name of a distance function `written`, `s.n` or `n`, of schema
/// `schema` if it names none.
fn distance_name(written: &str, schema: &str) -> QualifiedName {
    match written.split_once('.') {
        Some((auth, ident)) => QualifiedName { auth: auth.trim().to_owned(), ident: ident.trim().to_owned() },
        None => QualifiedName { auth: schema.to_owned(), ident: written.trim().to_owned() },
    }
}

/// The distance function of SQL `written` names, as `schema` reads it, if
/// one is (design document §8).
pub(super) fn distance_routine<'d>(db: &'d State, written: &str, schema: &str) -> Option<&'d SqlRoutineDesc> {
    let name = distance_name(written, schema);
    db.catalog.routines_named(&name).into_iter().find(|r| r.distance.is_some())
}

/// A table of the parameters of `desc`, a row of which its body is bound
/// over.
fn parameters_of(desc: &SqlRoutineDesc) -> TableDesc {
    let column = |p: &crate::catalog::SqlParameter| ColumnDesc {
        name: p.name.clone(),
        data_type: p.data_type.clone(),
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    TableDesc {
        name: desc.name.clone(),
        columns: desc.parameters.iter().map(column).collect(),
        constraints: Vec::new(),
        periods: Vec::new(),
        typed: None,
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// The metric the distance function of SQL `desc` is: its body, bound over
/// a row of its two parameters as a generation expression is over its
/// table's, evaluated for each pair of vectors (design document §8). Its
/// name is its qualified name, `S.N`.
///
/// # Errors
///
/// SQLCODE -103 if its body does not bind, contains a `<query
/// expression>` or a potential source of non-determinism, or is no number.
pub(crate) fn sql_metric(db: &State, desc: &SqlRoutineDesc) -> Result<Arc<dyn VectorMetric>> {
    let declared =
        desc.distance.as_ref().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {} is no distance function", desc.name)))?;
    let value = Parser::for_stored(&declared.expression)?.generation_expression()?;
    let mut queried = false;
    value.for_each_subquery(&mut |_| queried = true);
    if queried {
        return Err(rule(format!("[EXTENSION] the body of distance function {} contains no <query expression>", desc.name)));
    }
    let table = parameters_of(desc);
    let params = Params::new();
    let owner = crate::catalog::owner_of(&*db.catalog, &desc.name.auth);
    let cx = Context { purpose: Purpose::Generation, schema: &desc.schema, horizon: &desc.horizon, ..Context::new(db, owner, &params) };
    let item = FromItem::alone(desc.name.clone(), 0, (table.clone(), Source::Table));
    let visible = (0..table.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    let scope = Scope::Rows("[EXTENSION] a distance function's body is over one pair of vectors, so it contains no set function");
    let (e, t) = binder.value_expression(&value, scope)?;
    if !t.is_numeric() {
        return Err(rule(format!("[EXTENSION] the body of distance function {} is a number, not of {t}", desc.name)));
    }
    let call: DistanceFn = Arc::new(move |a, b| {
        let row = vec![Vector::of(a).value()?, Vector::of(b).value()?];
        let v = eval(&e, &row, &NoSubqueries)?;
        Ok(v.as_f64())
    });
    let formats = if declared.formats.is_empty() {
        Element::ALL.to_vec()
    } else {
        declared.formats.iter().filter_map(|c| Element::of_code(*c)).collect()
    };
    let properties = MetricProperties {
        symmetric: declared.symmetric,
        non_negative: declared.non_negative,
        triangle: declared.triangle,
        normalized_as: None,
    };
    let metric = CallbackMetric::new(&format!("{}.{}", desc.name.auth, desc.name.ident), formats, properties, call)?;
    Ok(Arc::new(metric))
}
