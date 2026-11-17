//! [ISO/IEC 9075-2:2023 §11.32 SR23m to SR23t] The query of a
//! referenceable view: one `<query specification>` of one table, its basis
//! table, named with ONLY if it is referenceable, neither grouped nor of an
//! aggregate function; of a view that is no subview, its self-referencing
//! column's values made from a candidate key of the basis table \[SR23s\];
//! of a subview, a basis table that is a proper subtable or subview of its
//! superview's, and a column reference wherever its superview's query has
//! one \[SR23t\].
//!
//! SR23t ii and SR23t iv to SR23t viii constrain a subview of a superview
//! that is effectively updatable or WITH CHECK OPTION, which no
//! referenceable view is here (`views/referenceable.rs`).

use crate::ast::{
    ColumnSpecification, QueryExpression, QuerySpecification, SelectList, SelectSublist, Subquery, TableReference, ValueExpression,
};
use crate::binder::{Bound, qualify};
use crate::catalog::{
    ConstraintKind, QualifiedName, RefRepresentation, StructuredDesc, TypedDesc, ViewDesc, proper_subtables, proper_supertables,
};
use crate::db::{State, api, rule};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;
use crate::value::DataType;

/// \[DESIGN\] The most queries nested in one another, or casts in one
/// another, a referenceable view's query is searched through for an
/// aggregate function or a leaf column (RULES.md R-CF-6).
const MAX_NESTING: usize = 256;

/// [ISO/IEC 9075-2:2023 §11.32 SR23o] The basis table of a referenceable
/// view's query, and the query.
struct Basis<'q> {
    /// The `<query specification>`.
    spec: &'q QuerySpecification,
    /// The basis table.
    table: QualifiedName,
    /// Its columns: the name the query sees each by, and its own.
    columns: Vec<(String, String)>,
}

/// An item of the select list of a referenceable view's query, one for
/// each of its columns.
#[derive(Default)]
struct Item {
    /// Of a column reference, the column name it is written with.
    written: Option<String>,
    /// Of a column reference, or a cast whose leaf column it is, the
    /// column of the basis table.
    leaf: Option<usize>,
    /// Whether it is a `<cast specification>`.
    cast: bool,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.32 SR23m to SR23t] That `query`, its names
    /// of schema `names`, bound as `bound`, is the query of a referenceable
    /// view of type `st` made typed by `typed`, of the direct superview
    /// `sup` if it is a subview.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks.
    pub(super) fn basis_rules(
        &self,
        query: &QueryExpression,
        names: &str,
        (st, typed): (&StructuredDesc, &TypedDesc),
        (sup, bound): (Option<ViewDesc>, &Bound),
    ) -> Result<()> {
        let basis = self.basis(query, names)?;
        if aggregated_spec(basis.spec, 0)? {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.32 SR23r] the query of a referenceable view contains no <aggregate function>"));
        }
        let items = basis.items();
        match sup {
            None => self.key_rules(&basis, &items, (st, typed), bound),
            Some(sv) => self.subview_rules(&basis, &items, &sv, typed),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23m to SR23q] The basis table of
    /// `query`, its names of schema `names`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `query` is no single `<query specification>` of one
    /// table named, with ONLY if referenceable, neither grouped nor of a
    /// HAVING clause.
    fn basis<'q>(&self, query: &'q QueryExpression, names: &str) -> Result<Basis<'q>> {
        let Some(spec) = query.spec() else {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.32 SR23m] the query of a referenceable view is a single <query specification>"));
        };
        let [only] = spec.table_expression.from.as_slice() else {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §11.32 SR23n] the <from clause> of a referenceable view has a single <table reference>",
            ));
        };
        let TableReference::Table(t) = only else {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.32 SR23o] the <table reference> of a referenceable view is a <table name>"));
        };
        let table = qualify(&t.table, names);
        let (own, referenceable): (Vec<String>, bool) = match (self.catalog.table(&table), self.catalog.view(&table)) {
            (Some(d), _) => (d.columns.iter().map(|c| c.name.clone()).collect(), d.typed.is_some()),
            (None, Some(v)) => (v.columns.iter().map(|c| c.name.clone()).collect(), v.typed.is_some()),
            (None, None) => return Err(api(format!("[DESIGN] the basis table {table} of a referenceable view is not defined"))),
        };
        if referenceable && !t.only {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23p] {table} is a referenceable table, so the view names it with ONLY"
            )));
        }
        if spec.table_expression.group_by.is_some() || spec.table_expression.having.is_some() {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.32 SR23q] the query of a referenceable view has no GROUP BY or HAVING clause"));
        }
        let seen = t.columns.clone().unwrap_or_else(|| own.clone());
        Ok(Basis { spec, table, columns: seen.into_iter().zip(own).collect() })
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23s] Of a referenceable view that is no
    /// subview, of type `st` made typed by `typed`, whose query reads
    /// `basis` by `items` and is bound as `bound`: of a user-defined
    /// reference representation, its first column a cast of a column that
    /// is a candidate key, to REF(ST); of a derived one, each derivational
    /// attribute's column a column reference, a candidate key among them.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it breaks one of those rules.
    fn key_rules(&self, basis: &Basis<'_>, items: &[Item], (st, typed): (&StructuredDesc, &TypedDesc), bound: &Bound) -> Result<()> {
        let keys = self.candidate_keys(basis);
        let table = &basis.table;
        if !matches!(typed.generation, crate::ast::RefGeneration::User) {
            return derived_keys(self, (st, items), (&keys, table));
        }
        if !keys.iter().any(|k| k.len() == 1) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.32 SR23s i 1] {table} has no candidate key of a single column")));
        }
        let rc = items.first().filter(|i| i.cast).and_then(|i| i.leaf);
        if !rc.is_some_and(|rc| keys.iter().any(|k| k.as_slice() == [rc])) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23s i 3] the first column of the view is a cast of the column of a candidate key of {table} of a single column"
            )));
        }
        let first = bound.columns.first().map(|c| &c.data_type);
        if !matches!(first, Some(DataType::Reference(r)) if r.referenced.name.schema == st.name.auth && r.referenced.name.name == st.name.ident)
        {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.32 SR23s i 4] the first column of the view is of REF({})", st.name)));
        }
        Ok(())
    }

    /// The candidate keys of the basis table of `basis`, its UNIQUE and
    /// PRIMARY KEY constraints' and those of its proper supertables, each
    /// the positions of its columns in the table.
    fn candidate_keys(&self, basis: &Basis<'_>) -> Vec<Vec<usize>> {
        let position = |name: &str| basis.columns.iter().position(|(_, own)| own == name);
        let tables = std::iter::once(basis.table.clone()).chain(proper_supertables(&*self.catalog, &basis.table));
        let mut keys = Vec::new();
        for d in tables.filter_map(|t| self.catalog.table(&t)) {
            for c in &d.constraints {
                if let ConstraintKind::Unique { columns, .. } = &c.kind {
                    let key: Option<Vec<usize>> = columns.iter().map(|&k| d.columns.get(k).and_then(|col| position(&col.name))).collect();
                    keys.extend(key);
                }
            }
        }
        keys
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23t i, iii] Of a subview whose query
    /// reads `basis` by `items`, made typed by `typed`, of the direct
    /// superview `sv`: its basis table a proper subtable or subview of
    /// `sv`'s, and its column of each attribute of `sv`'s type a column
    /// reference of the column name of `sv`'s, if that is one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it breaks either rule.
    fn subview_rules(&self, basis: &Basis<'_>, items: &[Item], sv: &ViewDesc, typed: &TypedDesc) -> Result<()> {
        let query = Parser::new(&sv.query)?.of_definition().view_query()?;
        let of_super = self.basis(&query, &sv.names)?;
        if !proper_subtables(&*self.catalog, &of_super.table).contains(&basis.table) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23t i] {} is no proper subtable or subview of {}, the basis table of {}",
                basis.table, of_super.table, sv.name
            )));
        }
        let m = usize::from(matches!(typed.generation, crate::ast::RefGeneration::User));
        let attributes = sv.columns.len().saturating_sub(1);
        for (j, above) in of_super.items().into_iter().enumerate().skip(m).take(attributes) {
            let Some(name) = above.written else { continue };
            if items.get(j).and_then(|i| i.written.as_deref()) != Some(name.as_str()) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.32 SR23t iii] column {} of the view is a column reference to {name} of {}, as that of {} is",
                    j.saturating_add(1).saturating_sub(m),
                    basis.table,
                    sv.name
                )));
            }
        }
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23s ii] Of a view of type `st` whose
/// references are derived, whose query's items are `items`: each column of
/// a derivational attribute a column reference, and a candidate key of
/// `keys`, those of its basis table `table`, among their columns.
///
/// # Errors
///
/// SQLCODE -103 if either is not.
fn derived_keys(db: &State, (st, items): (&StructuredDesc, &[Item]), (keys, table): (&[Vec<usize>], &QualifiedName)) -> Result<()> {
    let maximal = db.catalog.structured_type(&st.maximal()).unwrap_or(st);
    let RefRepresentation::Derived(names) = &maximal.reference else {
        return Err(api(format!("[DESIGN] REF({}) has no derived representation", st.name)));
    };
    let mut underlying = Vec::new();
    for n in names {
        let item = st.attribute(n).and_then(|(i, _)| items.get(i)).filter(|i| i.written.is_some());
        let Some(leaf) = item.and_then(|i| i.leaf) else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23s ii 2] the column of derivational attribute {n} is a column reference"
            )));
        };
        underlying.push(leaf);
    }
    if !keys.iter().any(|k| k.iter().all(|c| underlying.contains(c))) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.32 SR23s ii 3] {table} has no candidate key among the columns of the derivational attributes ({})",
            names.join(", ")
        )));
    }
    Ok(())
}

impl Basis<'_> {
    /// The items of the select list of its query, one for each column: of
    /// `*` or `<qualifier>.*`, a column reference to each of its columns.
    fn items(&self) -> Vec<Item> {
        let all = || self.columns.iter().enumerate().map(|(i, (seen, _))| Item { written: Some(seen.clone()), leaf: Some(i), cast: false });
        match &self.spec.select_list {
            SelectList::Star => all().collect(),
            SelectList::Values(vs) => vs
                .iter()
                .flat_map(|v| match v {
                    SelectSublist::Derived(item) => vec![self.item(&item.value)],
                    SelectSublist::All(_) => all().collect(),
                    SelectSublist::Fields { .. } => vec![Item::default()],
                })
                .collect(),
        }
    }

    /// The item of the `<derived column>` of value `v`.
    fn item(&self, v: &ValueExpression) -> Item {
        let position = |c: &ColumnSpecification| self.columns.iter().position(|(seen, _)| *seen == c.column);
        if let ValueExpression::Column(c) = v {
            return Item { written: Some(c.column.clone()), leaf: position(c), cast: false };
        }
        if let ValueExpression::Cast { operand: Some(x), .. } = v {
            return Item { written: None, leaf: leaf_column(x).and_then(position), cast: true };
        }
        Item::default()
    }
}

/// [ISO/IEC 9075-2:2023 §6.13 SR5] The leaf column of a cast of `operand`:
/// the column it references, alone or through casts.
fn leaf_column(operand: &ValueExpression) -> Option<&ColumnSpecification> {
    let mut at = operand;
    for _ in 0..MAX_NESTING {
        if let ValueExpression::Column(c) = at {
            return Some(c);
        }
        let ValueExpression::Cast { operand: Some(x), .. } = at else { return None };
        at = x;
    }
    None
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23r] Whether `spec`, nested `depth`
/// queries deep, generally contains an `<aggregate function>`: in its
/// select list, its WHERE clause, or a query nested in them or in its FROM
/// clause.
///
/// # Errors
///
/// SQLCODE -900 if queries are nested more than `MAX_NESTING` deep.
fn aggregated_spec(spec: &QuerySpecification, depth: usize) -> Result<bool> {
    if depth > MAX_NESTING {
        let msg = format!("[DESIGN] a referenceable view's query nests at most {MAX_NESTING} queries (RULES.md R-CF-6)");
        return Err(SqlError::new(Code::Unsupported, msg));
    }
    let mut values: Vec<&ValueExpression> = Vec::new();
    if let SelectList::Values(vs) = &spec.select_list {
        for v in vs {
            match v {
                SelectSublist::Derived(item) => values.push(&item.value),
                SelectSublist::Fields { value, .. } => values.push(value),
                SelectSublist::All(_) => {}
            }
        }
    }
    let mut nested: Vec<&Subquery> = Vec::new();
    for v in &values {
        v.for_each_subquery(&mut |s| nested.push(s));
    }
    if let Some(c) = &spec.table_expression.where_clause {
        c.for_each_value(&mut |v| values.push(v));
        c.for_each_subquery(&mut |s| nested.push(s));
    }
    nested
        .extend(spec.table_expression.from.iter().filter_map(|r| if let TableReference::Derived(d) = r { Some(&*d.query) } else { None }));
    if values.iter().any(|v| v.any(&|x| matches!(x, ValueExpression::SetFunction(_)))) {
        return Ok(true);
    }
    for s in nested {
        if aggregated_query(&s.query, depth.saturating_add(1))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23r] Whether the query `q`, nested
/// `depth` queries deep, generally contains an `<aggregate function>`.
///
/// # Errors
///
/// As `aggregated_spec`.
fn aggregated_query(q: &QueryExpression, depth: usize) -> Result<bool> {
    match q.body() {
        QueryExpression::Spec(s) => aggregated_spec(s, depth),
        QueryExpression::SetOp(op) => {
            Ok(aggregated_query(&op.left, depth.saturating_add(1))? || aggregated_query(&op.right, depth.saturating_add(1))?)
        }
        QueryExpression::Values(_) | QueryExpression::With(_) | QueryExpression::Ordered(_) => Ok(false),
    }
}
