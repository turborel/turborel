//! [X3.135-1986 §6.5] `<view definition>`: the rules of a view and what the
//! catalog keeps of it. A referenceable view's are `views/referenceable.rs`.

mod referenceable;

use std::sync::Arc;

use super::{State, rule};
use crate::ast::ViewDefinition;
use crate::binder::{Bound, BoundView, Context, Invoked, bind_view, underlying};
use crate::catalog::{ColumnDesc, Horizon, QualifiedName, TemporaryKind, ViewDesc, owner_of};
use crate::error::{At, Result};
use crate::params::Params;

impl State {
    /// [X3.135-1986 §6.5] Records each view of `views`, of the schema
    /// `auth`, its names of schema `names` [ISO/IEC 9075:1992 §5.4 SR8], in
    /// turn in the catalog, so a view can name the ones before it. The
    /// caller has put the schema's tables in the catalog, and puts the
    /// catalog back if any view breaks a rule.
    pub(super) fn define_views(&mut self, (auth, names): (&str, &str), views: &[ViewDefinition]) -> Result<()> {
        for v in views {
            // [ISO/IEC 9075:1992 §18.1 GR3g] its errors about the view it defines.
            let about = (auth.to_owned(), v.name.ident.clone());
            let desc = self.describe_view((auth, names), v).map_err(|e| e.or_about_table(about)).or_at(v.name.at)?;
            let mut catalog = self.catalog.duplicate();
            catalog.add_views(vec![desc]);
            self.catalog = Arc::from(catalog);
        }
        Ok(())
    }

    /// [X3.135-1986 §6.5 SR1-SR9] The description of the view `v` of the
    /// schema `auth`, its names of schema `names`.
    pub(super) fn describe_view(&self, (auth, names): (&str, &str), v: &ViewDefinition) -> Result<ViewDesc> {
        if let Some(a) = v.name.auth.as_ref().filter(|a| *a != auth) {
            return Err(rule(format!("[X3.135-1986 §6.5 SR1] view {a}.{} is not in schema {auth}", v.name.ident)));
        }
        let name = QualifiedName { auth: auth.to_owned(), ident: v.name.ident.clone() };
        if self.catalog.table(&name).is_some() || self.catalog.view(&name).is_some() {
            return Err(rule(format!("[X3.135-1986 §6.5 SR2] {name} is defined twice")));
        }
        // [X3.135-1986 §6.6 SR3a i] its tables are read with the privileges
        // of the schema's owner.
        // [ISO/IEC 9075-2:2023 §4.35.2] the routines it invokes, its
        // subject routines from now on.
        let (params, invoked) = (Params::new(), Invoked::default());
        let cx = Context { schema: names, invoked: Some(&invoked), ..Context::new(self, owner_of(&*self.catalog, auth), &params) };
        let BoundView { bound, grouped, updates, reads } = bind_view(cx, &v.query)?;
        let (columns, typed, derivational) = match &v.typed {
            Some(_) => {
                let r = self.referenceable_view((&name, names, cx.auth), v, &bound)?;
                (r.columns, Some(r.typed), r.derivational)
            }
            None => (regular_columns(&name, v, &bound)?, None, None),
        };
        // [DESIGN] A referenceable view is read-only (`views/referenceable.rs`).
        let updates = updates.filter(|_| typed.is_none());
        if v.check_option.is_some() && updates.is_none() {
            return Err(rule(format!("[X3.135-1986 §6.5 SR9] {name} is WITH CHECK OPTION, so must be updatable")));
        }
        if let (Some(_), Some((under, _))) = (v.check_option, &updates) {
            self.unchecked_under(&name, under)?;
        }
        // [ISO/IEC 9075:1992 §11.11 GR2, note] the columns it sees of each table it reads, as they are now.
        // [ISO/IEC 9075:1992 §11.19 SR5] no declared local temporary table.
        let declared =
            |t: &QualifiedName| self.catalog.table(t).is_some_and(|d| d.temporary.is_some_and(|d| d.kind == TemporaryKind::Declared));
        if let Some(t) = reads.iter().find(|t| declared(t)) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.19 SR5] view {name} names {t}, a declared local temporary table")));
        }
        // [ISO/IEC 9075:1992 §11.19 GR6] what its owner's REFERENCES on it are found from.
        let underlying = Some(referenceable::prefixed(underlying(cx, &v.query)?, derivational.as_deref()));
        // [ISO/IEC 9075-14:2006 draft §4.2.6] and the registered XML
        // Schemas it indicates.
        let horizon =
            Horizon::of(&*self.catalog, &reads).invoking(invoked.take()).indicating(invoked.take_indicated()).typing(invoked.take_typed());
        let (query, names) = (v.text.clone(), names.to_owned());
        Ok(ViewDesc { typed, name, columns, query, names, check_option: v.check_option, grouped, updates, reads, horizon, underlying })
    }
}

/// [X3.135-1986 §6.5 SR7] The columns of the regular view `name` defined
/// by `v`, whose query is `bound`: those of the query, as named.
///
/// # Errors
///
/// As `view_column_names`.
fn regular_columns(name: &QualifiedName, v: &ViewDefinition, bound: &Bound) -> Result<Vec<ColumnDesc>> {
    let titles = view_column_names(name, v.columns.as_deref(), bound.columns.iter().map(|c| c.name.clone()).collect())?;
    let columns = titles.into_iter().zip(&bound.columns).map(|(column, c)| ColumnDesc {
        name: column,
        data_type: c.data_type.clone(),
        not_null: c.not_null,
        default: None,
        domain: None,
        collation: c.collation.clone(),
        generated: None,
    });
    Ok(columns.collect())
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.32 SR19] That no view the updatable view
    /// `name` WITH CHECK OPTION reads, `under` and each view under that in
    /// turn, its generally underlying tables, is trigger insertable-into,
    /// updatable or deletable: has an INSTEAD OF trigger.
    fn unchecked_under(&self, name: &QualifiedName, under: &QualifiedName) -> Result<()> {
        let mut at = under.clone();
        while let Some(view) = self.catalog.view(&at) {
            if let Some(t) = self.catalog.triggers_on(&at).iter().find(|t| t.instead()) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.32 SR19] {name} is WITH CHECK OPTION, so reads no view with an INSTEAD OF trigger, as {at} has {}",
                    t.name
                )));
            }
            let Some((next, _)) = &view.updates else { break };
            at = next.clone();
        }
        Ok(())
    }
}

/// [X3.135-1986 §6.5 SR4-SR7] The names of the columns of the view `name`:
/// those of its `<view column list>` `list`, one for each of the query's
/// columns, none twice; otherwise the query's, if each is named and none
/// twice.
fn view_column_names(name: &QualifiedName, list: Option<&[String]>, query: Vec<Option<String>>) -> Result<Vec<String>> {
    let Some(list) = list else {
        let named: Option<Vec<String>> = query.into_iter().collect();
        let Some(named) = named.filter(|n| distinct(n)) else {
            return Err(rule(format!(
                "[X3.135-1986 §6.5 SR4] {name} needs a <view column list>: its query has an unnamed column or two of one name"
            )));
        };
        return Ok(named);
    };
    if !distinct(list) {
        return Err(rule(format!("[X3.135-1986 §6.5 SR5] the <view column list> of {name} names a column twice")));
    }
    if list.len() != query.len() {
        let names = if list.len() == 1 { "name" } else { "names" };
        return Err(rule(format!(
            "[X3.135-1986 §6.5 SR6] {name} lists {} column {names} for a query of degree {}",
            list.len(),
            query.len()
        )));
    }
    Ok(list.to_vec())
}

/// Whether no two of `names` are the same.
fn distinct(names: &[String]) -> bool {
    names.iter().enumerate().all(|(i, n)| !names.iter().skip(i.saturating_add(1)).any(|m| m == n))
}
