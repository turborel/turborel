//! \[EXTENSION\] The arguments a registered function declares a call of
//! it is given after its own (RULES.md D87): the name of the index that
//! ranks the column its first argument reads, as SQL/MM Full-Text's Rank
//! is given it (`ScalarFunction::ranked_by_index`), then the ARRAY of a
//! query read in the statement's snapshot, as Contains and Rank are given
//! their thesauri's relations (`ScalarFunction::appended_query`).

use super::views::Source;
use super::{Binder, Scope};
use crate::ast::ValueExpression;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::error::{Result, SqlError};
use crate::functions::ScalarFunction;
use crate::index::FULLTEXT_METHOD;
use crate::logical::Expr;
use crate::parser::Parser;
use crate::value::{CharUnits, DataType, Value};

impl Binder<'_> {
    /// The arguments `f` declares a call of `args`, bound as `exprs`, is
    /// given after them, bound.
    ///
    /// # Errors
    ///
    /// As `appended`.
    pub(super) fn appended_arguments(
        &self,
        f: &dyn ScalarFunction,
        (args, exprs, types): (&[ValueExpression], &[Expr], &[DataType]),
        scope: Scope<'_>,
    ) -> Result<Vec<(Expr, DataType)>> {
        let mut out = Vec::new();
        if f.ranked_by_index()
            && let Some(index) = self.ranking_index(args.first())
        {
            let length = u32::try_from(index.chars().count()).unwrap_or(u32::MAX).max(1);
            out.push((Expr::Const(Value::Char(index.into())), DataType::Varying { length, units: CharUnits::Characters }));
        }
        let given: Vec<Expr> = exprs.iter().cloned().chain(out.iter().map(|(e, _)| e.clone())).collect();
        out.extend(self.appended(f, (&given, types), scope)?);
        Ok(out)
    }

    /// The full-text index of the base table's column `first` is, if it is
    /// a column reference of a table of this query that has one, or a
    /// hybrid index of it: of several, the first by name.
    fn ranking_index(&self, first: Option<&ValueExpression>) -> Option<String> {
        let Some(ValueExpression::Column(cs)) = first else { return None };
        if cs.catalog.is_some() {
            return None;
        }
        let (item, column, _) = match &cs.qualifier {
            Some(q) => self.qualified(q, &cs.column).ok()?,
            None => self.unqualified(&cs.column).ok()?,
        };
        let Source::Table = item.source else { return None };
        // A hybrid index's text part ranks its first column (`index/vector/hybrid.rs`).
        let hybrid = |ix: &IndexDesc| ix.method == crate::index::vector::hybrid::HYBRID_METHOD && ix.columns.first() == Some(&column);
        let ranking = |ix: &&IndexDesc| ix.method == FULLTEXT_METHOD && ix.columns.as_slice() == [column] || hybrid(ix);
        let indexes = self.db.catalog.indexes(&item.table);
        let own = indexes.into_iter().filter(ranking).map(|ix| ix.name.clone()).min();
        // [EXTENSION] a partitioned table's index ranks with its partitions' together (RULES.md D158).
        own.or_else(|| {
            let templates = &self.db.catalog.partitioning().key(&item.table)?.templates;
            templates.iter().filter(|t| t.method == FULLTEXT_METHOD && t.columns.as_slice() == [column]).map(|t| t.name.clone()).min()
        })
    }

    /// The argument `f` declares a call of `exprs` is given last
    /// (`ScalarFunction::appended_query`), bound: ARRAY of its query, if the
    /// tables it reads are all there and the first has the columns it names
    /// (RULES.md D87).
    ///
    /// # Errors
    ///
    /// As the query's binding, placed nowhere in the statement, since it is
    /// not written in it.
    fn appended(
        &self,
        f: &dyn ScalarFunction,
        (exprs, types): (&[Expr], &[DataType]),
        scope: Scope<'_>,
    ) -> Result<Option<(Expr, DataType)>> {
        let constants = exprs.iter().map(constant).collect::<Vec<_>>();
        let Some(appended) = f.catalog_query(&constants, types, &*self.db.catalog) else { return Ok(None) };
        // A table of no schema is of the default one (`crate::binder::qualify`).
        let auth_of = |auth: &String| if auth.is_empty() { self.schema.to_owned() } else { auth.clone() };
        let there = |(auth, ident): &(String, String)| {
            self.db.catalog.table(&QualifiedName { auth: auth_of(auth), ident: ident.clone() }).is_some()
        };
        let has = |(auth, ident): &(String, String)| {
            let table = self.db.catalog.table(&QualifiedName { auth: auth_of(auth), ident: ident.clone() });
            table.is_some_and(|t| appended.columns.iter().all(|c| t.columns.iter().any(|d| d.name == *c)))
        };
        if !appended.tables.iter().all(there) || !appended.tables.first().is_none_or(has) {
            return Ok(None);
        }
        let unplaced = |e: SqlError| SqlError { pos: None, ..e };
        let array = Parser::new(&format!("ARRAY({})", appended.query)).and_then(|mut p| p.generation_expression()).map_err(unplaced)?;
        self.value_expression(&array, scope).map(Some).map_err(unplaced)
    }
}

/// The constant `e` is, or stores in its declared type, as a pattern
/// string is stored as an FT_PATTERN argument.
fn constant(e: &Expr) -> Option<&Value> {
    if let Expr::Const(v) = e {
        return Some(v);
    }
    let Expr::Call(f, args) = e else { return None };
    let [Expr::Const(v)] = args.as_slice() else { return None };
    (f.0.name() == super::arguments::ASSIGN).then_some(v)
}
