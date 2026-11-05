//! \[EXTENSION\] The table of an assertion's kept totals (`kept.rs`) made
//! with the assertion, filled as it is checked made, dropped with it, and
//! put right of a change past what a deferred check holds (RULES.md D51).

use std::sync::Arc;

use std::collections::BTreeMap;

use super::forms::{Form, Kind, Source, forms_of};
use super::kept::{Register, Totals, WIDTH, keeps, kept_desc, kept_name, parse_row};
use crate::catalog::{Catalog, QualifiedName, owner_of};
use crate::db::State;
use crate::db::alter::Made;
use crate::db::ddl::Dropped;
use crate::error::Result;
use crate::logical::LogicalPlan;
use crate::storage::Row;
use crate::system::DEFINITION_SCHEMA;
use crate::system::text;
use crate::txn::NO_TRANSACTION;
use crate::txn::Txn;
use crate::value::{Decimal, Value};

/// Of an assertion keeping totals, their table, and each form keeping
/// them with its place among the forms.
type KeptForms = (QualifiedName, Vec<(usize, Form)>);

impl State {
    /// \[EXTENSION\] The rows of INFORMATION_SCHEMA.ASSERTION_TOTALS that
    /// `auth` sees (`system/totals.rs`): of each assertion of a schema it
    /// owns, in order of name, each form keeping totals, as committed.
    ///
    /// # Errors
    ///
    /// An assertion's condition does not bind; its totals' reads'.
    pub(crate) fn assertion_totals(&self, auth: &str) -> Result<Vec<Row>> {
        let catalog = self.catalog_name();
        let mut assertions: Vec<QualifiedName> = self
            .catalog
            .assertions()
            .into_iter()
            .filter(|a| owner_of(&*self.catalog, &a.name.auth) == auth)
            .map(|a| a.name.clone())
            .collect();
        assertions.sort_by_key(ToString::to_string);
        let mut out = Vec::new();
        for name in assertions {
            let Some((kept, forms)) = self.kept_forms(&name)? else { continue };
            // Committed, outside any transaction.
            let mut by_form: BTreeMap<usize, (Totals, i128)> = BTreeMap::new();
            for found in Arc::clone(self.storage_of(&kept)?).scan(NO_TRANSACTION, &(0..WIDTH).collect::<Vec<_>>()) {
                let (form, totals) = parse_row(&found?.1)?;
                let (sum, rows) = by_form.entry(form).or_default();
                *sum = sum.plus(totals)?;
                *rows = rows.saturating_add(1);
            }
            for (at, form) in &forms {
                let (totals, rows) = by_form.get(at).copied().unwrap_or_default();
                out.push(totals_row((&catalog, &name), (*at, form), (&totals, rows)));
            }
        }
        Ok(out)
    }

    /// Of assertion `name`, whose bound query is `plan`: if a form of it
    /// keeps totals, their table, made in `catalog` of what `made` holds
    /// for it, if given, and each table it reads marked changed, so a
    /// writer whose snapshot predates it changes them only once it is
    /// published (`State::prepare_write`). What was made.
    ///
    /// # Errors
    ///
    /// The table's storage cannot be made.
    pub(super) fn make_kept(
        &mut self,
        catalog: &mut dyn Catalog,
        (name, plan): (&QualifiedName, &LogicalPlan),
        made: Option<&Made>,
    ) -> Result<Option<Made>> {
        let forms = forms_of(plan, &self.known()).unwrap_or_default();
        if !forms.iter().any(keeps) {
            return Ok(None);
        }
        let kept = kept_name(name);
        // Another of its name, dropped, had a generation of its own.
        let generation = self.retired.get(&kept).map_or(0, |g| g.saturating_add(1));
        let desc = kept_desc(kept, generation);
        if catalog.schema_exists(DEFINITION_SCHEMA) {
            catalog.add_table(desc.clone());
        } else {
            catalog.create_schema(DEFINITION_SCHEMA, DEFINITION_SCHEMA, vec![desc.clone()]);
        }
        let built = self.place_table(catalog, &desc, made)?;
        for table in plan.tables() {
            self.lock_of(&table)?.definitions_changed(self.generation);
        }
        Ok(Some(built))
    }

    /// The table of assertion `name`'s kept totals, if it keeps any,
    /// dropped from `catalog`, its storage taken into `dropped`.
    pub(super) fn drop_kept(&mut self, catalog: &mut dyn Catalog, name: &QualifiedName, dropped: &mut Dropped) {
        let kept = kept_name(name);
        if catalog.table(&kept).is_none() {
            return;
        }
        catalog.remove_table(&kept);
        let generation = self.generation_of(&kept);
        self.retired.entry(kept.clone()).and_modify(|g| *g = (*g).max(generation)).or_insert(generation);
        dropped.storage.extend(self.storage.remove(&kept));
        dropped.names.push(kept);
    }

    /// The forms of assertion `name` that keep totals, each with its place
    /// among the forms, and their table: none if it keeps none.
    ///
    /// # Errors
    ///
    /// Its condition does not bind.
    pub(super) fn kept_forms(&self, name: &QualifiedName) -> Result<Option<KeptForms>> {
        let kept = kept_name(name);
        let Some(a) = self.catalog.assertion(name).filter(|_| self.catalog.table(&kept).is_some()) else { return Ok(None) };
        let forms = forms_of(&self.assertion_plan(a)?, &self.known()).unwrap_or_default();
        Ok(Some((kept, forms.into_iter().enumerate().filter(|(_, f)| keeps(f)).collect())))
    }

    /// [ISO/IEC 9075:1992 §11.34] The totals assertion `name`, just made
    /// and checked, keeps, of its tables as transaction `tx` sees them,
    /// written to their table.
    ///
    /// # Errors
    ///
    /// Reading the tables', or writing the totals'.
    pub(in crate::db) fn start_totals(self: &Arc<Self>, tx: &Txn, name: &QualifiedName) -> Result<()> {
        let Some((kept, forms)) = self.kept_forms(name)? else { return Ok(()) };
        for (at, form) in &forms {
            let Source::Table(table) = &form.source else { continue };
            self.read_lock(tx, table)?;
            let totals = self.whole_totals(tx, (form, table))?;
            self.append_totals(tx, (&kept, *at), (totals, form))?;
        }
        Ok(())
    }

    /// The totals assertion `name` keeps put right, of a change past what
    /// a deferred check held: each form's of its table whole, less what is
    /// kept, every row of the form read recorded.
    ///
    /// # Errors
    ///
    /// Reading the tables', or writing the totals'.
    pub(super) fn rebuild_totals(self: &Arc<Self>, tx: &Txn, name: &QualifiedName) -> Result<()> {
        let Some((kept, forms)) = self.kept_forms(name)? else { return Ok(()) };
        for (at, form) in &forms {
            let Source::Table(table) = &form.source else { continue };
            let whole = self.whole_totals(tx, (form, table))?;
            let kept_now = self.read_totals(tx, (&kept, *at), Register::Form)?;
            self.append_totals(tx, (&kept, *at), (whole.minus(kept_now)?, form))?;
        }
        Ok(())
    }
}

/// A row of INFORMATION_SCHEMA.ASSERTION_TOTALS, of catalog `catalog`:
/// form `at` of assertion `name`, its totals as committed, kept in `rows`
/// rows. Of nulls alone, no count of values; of them and of a count, no
/// sum.
fn totals_row((catalog, name): (&str, &QualifiedName), (at, form): (usize, &Form), (totals, rows): (&Totals, i128)) -> Row {
    let number = |n: i128| Value::Exact(Decimal::new(n, 0));
    let (kind, of_values, summed) = match &form.kind {
        Kind::Total(t) => (if t.avg { "AVG" } else { "SUM" }, true, true),
        Kind::Count(..) => ("COUNT", true, false),
        Kind::NoRow | Kind::Bound | Kind::Reach(_) | Kind::Distinct(..) | Kind::Apart { .. } => ("NULLS", false, false),
    };
    let sum = totals.sum.unwrap_or(Decimal::new(0, 0));
    vec![
        text(catalog),
        text(&name.auth),
        text(&name.ident),
        number(i128::try_from(at).unwrap_or(i128::MAX).saturating_add(1)),
        text(kind),
        if of_values { number(totals.counted) } else { Value::Null },
        number(totals.nulled),
        if summed { text(&sum.to_string()) } else { Value::Null },
        number(rows),
    ]
}
