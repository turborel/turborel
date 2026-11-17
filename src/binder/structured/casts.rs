//! [ISO/IEC 9075-2:2023 §6.13 SR12, SR13, AR2, GR3, GR4] CAST of or to a
//! user-defined type by a user-defined cast: of a value whose type
//! precedence list holds the cast's source type, the first such, to the
//! cast's target, its cast function invoked; to a supertype, the value
//! itself (RULES.md D81).

use std::sync::Arc;

use super::super::invocation::held;
use super::super::{Binder, rule};
use crate::catalog::{CastDesc, owner_of};
use crate::error::{Code, Result, SqlError};
use crate::logical::Expr;
use crate::value::DataType;
use crate::value::precedence::position;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.13 SR12, GR3] CAST of `e`, of type `t`, to
    /// `target`, either a user-defined type: to a supertype, the value; by
    /// the appropriate user-defined cast, its function's result, cast to a
    /// target of no user-defined type [GR3b iii 2]; of a reference type,
    /// by a user-defined cast too [SR13c, GR4b]. `e` given back if there is
    /// no such cast of a distinct or a reference type, as its own rules
    /// take it (`cast_function`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a structured type is cast to neither a supertype
    /// nor by a user-defined cast [SR12]; -104 if EXECUTE is not held on
    /// the cast function [AR2b].
    pub(in crate::binder) fn user_cast(
        &self,
        (e, t): (Expr, DataType),
        target: &DataType,
    ) -> Result<std::result::Result<(Expr, DataType), Expr>> {
        if let (DataType::Structured(a), DataType::Structured(b)) = (&t, target)
            && a.is_subtype_of(&b.name)
        {
            return Ok(Ok((e, target.clone())));
        }
        let Some(cast) = self.appropriate_cast(&t, target) else {
            if matches!(t, DataType::Structured(_)) || matches!(target, DataType::Structured(_)) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §6.13 SR12] no user-defined cast takes {t} to {target}, nor is {target} a supertype of it"
                )));
            }
            return Ok(Err(e));
        };
        let invoked = self.user_cast_invocation(cast, e)?;
        let returns = invoked.1;
        if matches!(target, DataType::Structured(_) | DataType::Distinct(_)) || returns == *target {
            return Ok(Ok((invoked.0, target.clone())));
        }
        self.cast_bound((invoked.0, returns), target.clone()).map(Ok)
    }

    /// [ISO/IEC 9075-2:2023 §6.13 SR12, SR13c] The user-defined cast to
    /// `target` whose source type's designator is in the type precedence
    /// list of `t`, the first in it.
    pub(in crate::binder) fn appropriate_cast(&self, t: &DataType, target: &DataType) -> Option<&CastDesc> {
        let catalog: &dyn crate::catalog::Catalog = &*self.db.catalog;
        let to = catalog.casts().into_iter().filter(|c| c.target == *target);
        to.filter_map(|c| position(&c.source, t).map(|at| (at, c))).min_by_key(|(at, _)| *at).map(|(_, c)| c)
    }

    /// [ISO/IEC 9075-2:2023 §6.13 AR2b, GR3b ii] The invocation of the
    /// cast function of `cast` of the argument `e`, and its result type.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if EXECUTE on it is not held; -103 if it is gone.
    pub(in crate::binder) fn user_cast_invocation(&self, cast: &CastDesc, e: Expr) -> Result<(Expr, DataType)> {
        let r = self
            .db
            .catalog
            .routine(&cast.function)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.13 GR3b i] the cast function {} is gone", cast.function)))?;
        let who = self.who();
        if !(who.includes(owner_of(&*self.db.catalog, &r.name.auth)) || held(self.db, r, who)) {
            let msg = format!("[ISO/IEC 9075-2:2023 §6.13 AR2b] {} holds no EXECUTE on the cast function {}", who.auth, r.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        self.permitted(r)?;
        let invocation = self.bound_invocation(r, vec![e])?;
        let returns = invocation.result.clone();
        Ok((Expr::Invoked(Arc::new(invocation)), returns))
    }
}

/// [ISO/IEC 9075-2:2023 §4.13, §9.1 SR3, §9.2 SR3] The appropriate
/// user-defined cast function to assign a value of `t` to a site of
/// `target`, if `t` is not assignable to it: of the casts AS ASSIGNMENT,
/// implicitly invocable, whose result type is `target`, the one whose
/// parameter's designator comes first in the type precedence list of `t`.
pub fn assignment_cast<'c>(catalog: &'c dyn crate::catalog::Catalog, t: &DataType, target: &DataType) -> Option<&'c CastDesc> {
    if super::super::assignable(target, t) {
        return None;
    }
    let implicit = catalog.casts().into_iter().filter(|c| c.assignment && c.target == *target);
    implicit.filter_map(|c| position(&c.source, t).map(|at| (at, c))).min_by_key(|(at, _)| *at).map(|(_, c)| c)
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §9.1 GR2, §9.2 GR2] `e`, of type `t`, as it is
    /// assigned to a site of `target`: itself if `t` is assignable to it,
    /// else the invocation of the appropriate user-defined cast function of
    /// it (`assignment_cast`); none if there is neither.
    ///
    /// # Errors
    ///
    /// As `user_cast_invocation`'s.
    pub(in crate::binder) fn assigned(&self, (e, t): (Expr, &DataType), target: &DataType) -> Result<Option<Expr>> {
        if super::super::assignable(target, t) {
            return Ok(Some(e));
        }
        let Some(cast) = assignment_cast(&*self.db.catalog, t, target) else { return Ok(None) };
        Ok(Some(self.user_cast_invocation(cast, e)?.0))
    }

    /// The query `query`, of a row of the types `types`, each value
    /// assigned to a site of the type in its place of `targets` as
    /// `assigned` says; none if one is not assignable.
    ///
    /// # Errors
    ///
    /// As `assigned`'s.
    pub(in crate::binder) fn assigned_query(
        &self,
        query: &Arc<crate::logical::Subquery>,
        types: &[DataType],
        targets: &[&DataType],
    ) -> Result<Option<Arc<crate::logical::Subquery>>> {
        if types.iter().zip(targets).all(|(t, target)| super::super::assignable(target, t)) {
            return Ok(Some(Arc::clone(query)));
        }
        let mut exprs = Vec::with_capacity(types.len());
        for (i, (t, target)) in types.iter().zip(targets).enumerate() {
            let Some(e) = self.assigned((Expr::Column(i), t), target)? else { return Ok(None) };
            exprs.push(e);
        }
        let plan = crate::logical::LogicalPlan::Project { exprs, input: Box::new(query.plan.clone()) };
        Ok(Some(Arc::new(crate::logical::Subquery::new(plan, query.outer.clone()))))
    }
}
