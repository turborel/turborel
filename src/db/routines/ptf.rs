//! [ISO/IEC 9075-2:2023 §11.60 SR10, AR3] The definition of a polymorphic
//! table function: its routine descriptor's PTF part made, its component
//! procedures found and checked against the signatures §9.26 gives them,
//! and what its body, which is no statement, invokes: its component
//! procedures, its subject routines, which DROP and REVOKE follow
//! [§11.62 SR4b, GR5].

use super::super::{State, rule};
use super::body::BodyBound;
use crate::ast::{ParameterType, PrivilegeObject, PtfDefinition, RoutineDesignator, TableSemantics, ValueExpression};
use crate::binder::{Context, assignable, bind_value, held};
use crate::catalog::{
    DataAccess, GenericParameter, ParameterMode, PtfDesc, QualifiedName, SqlParameter, SqlRoutineDesc, owner_of, ptf_name_type,
    ptf_status_type, sole,
};
use crate::error::{At, Code, Result, SqlError};
use crate::params::Params;
use crate::value::DataType;

/// The component procedures of a polymorphic table function.
const COMPONENTS: [&str; 4] = ["describe", "start", "fulfill", "finish"];
/// The place of the describe component among `COMPONENTS`.
const DESCRIBE: usize = 0;
/// The place of the fulfill component among `COMPONENTS`.
const FULFILL: usize = 2;
/// The place of the finish component among `COMPONENTS`.
const FINISH: usize = 3;

/// [ISO/IEC 9075-2:2023 §9.26 SR2a] The parameter list a PTF component
/// procedure `component` (a place in `COMPONENTS`) conforms to, of the
/// function's public parameters `public` and private parameters
/// `private`, and of whether its describe component procedure is passed
/// the initial result row type descriptor area's name: each parameter's
/// mode and declared type.
fn signature(component: usize, (public, private): (&[SqlParameter], &[SqlParameter]), describes: bool) -> Vec<(ParameterMode, DataType)> {
    let name = || (ParameterMode::In, ptf_name_type());
    let mode = if component == FINISH { ParameterMode::In } else { ParameterMode::InOut };
    let mut out: Vec<(ParameterMode, DataType)> = private.iter().map(|p| (mode, p.data_type.clone())).collect();
    for p in public {
        match p.generic {
            None => out.push((ParameterMode::In, p.data_type.clone())),
            Some(GenericParameter::Descriptor) => out.push(name()),
            Some(GenericParameter::Table(g)) => {
                // [vii 1 A, B] its full row type's, or its cursor row type's.
                if component == DESCRIBE || component == FULFILL {
                    out.push(name());
                }
                if let TableSemantics::Set { .. } = g.semantics {
                    out.extend([name(), name()]);
                }
                // [vii 1 D, E] its requested row type's, or its cursor's.
                if component == DESCRIBE || component == FULFILL {
                    out.push(name());
                }
            }
        }
    }
    // [viii, ix] The text names it the type "of Pp", which is no parameter
    // here: read as VARCHAR(L), the type of the name it is passed.
    if component != DESCRIBE || describes {
        out.push(name());
    }
    out.push((ParameterMode::InOut, ptf_status_type()));
    out
}

/// An exception of [ISO/IEC 9075-2:2023 §11.60] `rule_of`.
fn broken(rule_of: &str, msg: &str) -> SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §11.60 {rule_of}] {msg}"))
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.60 SR10g, SR16] The SQL parameter of the
    /// `<parameter type>` `t`: of a generic table or descriptor parameter
    /// type, of the declared type of the name its component procedures
    /// are passed of it, VARCHAR(L).
    pub(super) fn ptf_parameter(&self, t: &ParameterType, (schema, owner): (&str, &str)) -> Result<(DataType, Option<GenericParameter>)> {
        match t {
            ParameterType::Data(t) => Ok((self.parameter_type(t, (schema, owner))?, None)),
            ParameterType::Table(g) => Ok((ptf_name_type(), Some(GenericParameter::Table(*g)))),
            ParameterType::Descriptor => Ok((ptf_name_type(), Some(GenericParameter::Descriptor))),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10] The PTF part of the descriptor of
    /// the function of PTF body `def`,
    /// its names without a schema in `schema`, of owner `owner`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a column's type or a private parameter's type is
    /// a domain [SR16], its result columns' names are alike, a private
    /// parameter's default is not assignable to it [SR10e iii], a
    /// component procedure designator designates no one procedure [SR10h],
    /// or no PTF describe component procedure is written where it is
    /// needed [SR10m].
    pub(super) fn ptf_desc(&self, def: &PtfDefinition, (schema, owner): (&str, &str)) -> Result<PtfDesc> {
        let columns = match &def.columns {
            Some(list) => {
                let mut out: Vec<(String, DataType)> = Vec::with_capacity(list.len());
                for (name, t, at) in list {
                    if out.iter().any(|(n, _)| n == name) {
                        return Err(broken("SR10f", &format!("the <table function column list> names column {name} twice")).or_at(*at));
                    }
                    out.push((name.clone(), self.parameter_type(t, (schema, owner)).or_at(*at)?));
                }
                Some(out)
            }
            None => None,
        };
        let mut private = Vec::with_capacity(def.private.len());
        for p in &def.private {
            let data_type = self.parameter_type(&p.data_type, (schema, owner)).or_at(p.at)?;
            if let Some((Some(v), _)) = &p.default {
                self.private_default(v, &data_type, (schema, owner)).or_at(p.at)?;
            }
            private.push(SqlParameter { name: p.name.clone(), data_type, mode: ParameterMode::InOut, generic: None, default: None });
        }
        if def.describe.is_none() && def.columns.is_none() && !def.only_pass_through {
            let msg = "a polymorphic table function of no <table function column list> nor ONLY PASS THROUGH has a PTF describe component procedure";
            return Err(broken("SR10m", msg));
        }
        let named = |d: &RoutineDesignator| self.component(d, (schema, owner));
        Ok(PtfDesc {
            columns,
            only_pass_through: def.only_pass_through,
            private,
            describe: def.describe.as_ref().map(named).transpose()?,
            start: def.start.as_ref().map(named).transpose()?,
            fulfill: named(&def.fulfill)?,
            finish: def.finish.as_ref().map(named).transpose()?,
        })
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10e iii] That the default `v` of a
    /// PTF private parameter of type `t` is store-assignable to it: bound
    /// with no table and no parameter.
    fn private_default(&self, v: &ValueExpression, t: &DataType, (schema, owner): (&str, &str)) -> Result<()> {
        let params = Params::default();
        let cx = Context { schema, ..Context::new(self, owner, &params) };
        let (_, found) = bind_value(cx, v)?;
        if !assignable(t, &found) {
            return Err(broken(
                "SR10e iii",
                &format!("a PTF private parameter's default of type {found} is not assignable to its type {t}"),
            ));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10h] The specific name of the one
    /// SQL-invoked procedure the component procedure designator `d`
    /// designates, its names without a schema in `schema`.
    fn component(&self, d: &RoutineDesignator, (schema, owner): (&str, &str)) -> Result<QualifiedName> {
        let object = if d.specific { PrivilegeObject::SpecificRoutine(d.routine_type) } else { PrivilegeObject::Routine(d.routine_type) };
        let found = self.sql_routine_named((owner, schema), (&d.name, object, d.parameters.as_deref(), None))?;
        match found {
            Some(r) if !r.function() && !r.polymorphic() => Ok(r.specific),
            Some(_) | None => {
                let msg = format!("{} designates no SQL-invoked procedure", crate::binder::qualify(&d.name, schema));
                Err(broken("SR10h", &msg).or_at(d.name.at))
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10h to SR10l, AR3] What the body of the
    /// polymorphic table function `desc` binds to: its component
    /// procedures, each conforming to its signature, possibly doing no
    /// more with SQL-data than the function may, deterministic as SR10k
    /// and SR10l say, and its owner holding EXECUTE on each.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is gone, or breaks a Syntax Rule; -104 if its
    /// owner does not hold EXECUTE on one.
    pub(super) fn bind_ptf_body(&self, desc: &SqlRoutineDesc, ptf: &PtfDesc) -> Result<BodyBound> {
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let who = sole(&*self.catalog, &owner);
        let describes = ptf.columns.is_none() && !ptf.only_pass_through;
        let written = [ptf.describe.as_ref(), ptf.start.as_ref(), Some(&ptf.fulfill), ptf.finish.as_ref()];
        let mut invokes = Vec::new();
        for (k, specific) in written.into_iter().enumerate() {
            let Some(specific) = specific else { continue };
            let what = COMPONENTS.get(k).copied().unwrap_or_default();
            let Some(r) = self.catalog.routine(specific) else {
                return Err(broken("SR10h", &format!("there is no PTF {what} component procedure {specific}")));
            };
            let wanted = signature(k, (&desc.parameters, &ptf.private), describes);
            let has: Vec<(ParameterMode, DataType)> = r.parameters.iter().map(|p| (p.mode, p.data_type.clone())).collect();
            if has != wanted || r.parameters.iter().any(|p| p.generic.is_some()) {
                let shown: Vec<String> = wanted.iter().map(|(m, t)| format!("{} {t}", m.name())).collect();
                let msg = format!("the parameters of PTF {what} component procedure {} do not conform to ({})", r.name, shown.join(", "));
                return Err(broken("SR10h", &msg));
            }
            Self::component_fits(desc, r, (k, what))?;
            if !held(self, r, who) {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.60 AR3] {owner} does not hold EXECUTE on PTF {what} component procedure {}", r.name);
                return Err(SqlError::new(Code::Privilege, msg));
            }
            invokes.push(r.specific.clone());
        }
        let grantable = self.execute_grantable((&owner, desc.module_name()), &[], &invokes);
        Ok(BodyBound {
            reads: Vec::new(),
            invokes,
            indicates: crate::catalog::Indicated::default(),
            typed: false,
            sequences: Vec::new(),
            grantable,
            columns: Vec::new(),
        })
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10j, SR10k, SR10l] That the PTF
    /// `what` component procedure `r` of `desc` does no more with SQL-data
    /// than `desc` may, and is deterministic if it is the describe
    /// component, or `desc` is.
    fn component_fits(desc: &SqlRoutineDesc, r: &SqlRoutineDesc, (k, what): (usize, &str)) -> Result<()> {
        match (r.access, desc.access) {
            (DataAccess::Modifies, _) => {
                return Err(broken("SR10j i", &format!("PTF {what} component procedure {} is MODIFIES SQL DATA", r.name)));
            }
            (DataAccess::Reads, DataAccess::Contains) => {
                let msg = format!("PTF {what} component procedure {} is READS SQL DATA, and the function CONTAINS SQL", r.name);
                return Err(broken("SR10j ii", &msg));
            }
            (DataAccess::Reads | DataAccess::Contains, DataAccess::Reads | DataAccess::Contains | DataAccess::Modifies) => {}
        }
        if k == DESCRIBE && !r.deterministic {
            return Err(broken("SR10k", &format!("PTF describe component procedure {} is not deterministic", r.name)));
        }
        if desc.deterministic && !r.deterministic {
            return Err(broken("SR10l", &format!("the function is deterministic, and PTF {what} component procedure {} is not", r.name)));
        }
        Ok(())
    }
}
