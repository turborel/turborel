//! [ISO/IEC 9075-2:2023 §6.21 to §6.24] Values reached by a reference,
//! bound: `r -> a`, `r -> m(args)` and DEREF(`r`), each an invocation of
//! a body that is a subquery of the scope table over the row of the
//! reference, as a function's body is (RULES.md D73, D81).
//!
//! The subquery names the reference and the scope table by names no
//! `<identifier>` is, so no column of the table hides the reference. A
//! DEREF reads each table of the scope's family with ONLY, the most
//! specific first, so its value's most specific type is the table's type.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::from::FromItem;
use super::super::subquery::Use;
use super::super::views::Source;
use super::super::{Binder, Scope, rule};
use super::{call, udt_name};
use crate::ast::{
    self, ColumnSpecification, NamedTable, Predicate, QueryExpression, QuerySpecification, SearchCondition, SelectItem, SelectList,
    SelectSublist, TableExpression, TableName, TableReference, ValueExpression,
};
use crate::catalog::{
    ColumnDesc, QualifiedName, StructuredDesc, TableDesc, TypedDesc, applicable, owner_of, proper_subtables, proper_supertables,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::Instanced;
use crate::logical::{Cond, Expr, Invocation, LogicalPlan};
use crate::value::types::Field;
use crate::value::{CompOp, DataType};

/// The correlation name of the row of the reference, which no
/// `<identifier>` is.
const REFERENCE: &str = "\u{0}REFERENCE";
/// The correlation name of the scope table, which no `<identifier>` is.
const SCOPED: &str = "\u{0}SCOPE";
/// The name a dereference's invocation is shown with.
const DEREFERENCE: &str = "DEREF";
/// The rule a set function in a dereference's subquery would break.
const ONE_ROW: &str = "[ISO/IEC 9075-2:2023 §6.22] a dereference reads one row, so it contains no set function of the reference";

/// A reference looked up in a typed table.
struct Looked<'r, 'a> {
    /// The reference, bound, and its declared type.
    reference: &'r (Expr, DataType),
    /// The self-referencing column's name.
    self_referencing: &'r str,
    /// Whose privileges it is looked up with, if not the statement's: the
    /// table's owner, of an operation of no Access Rules.
    owner: Option<&'a str>,
}

/// A reference bound, its type's referenced type and scope table.
struct Referenced<'a> {
    /// The reference, bound, and its declared type.
    value: (Expr, DataType),
    /// The referenced type.
    of: &'a StructuredDesc,
    /// The scope, a typed table or a referenceable view.
    table: QualifiedName,
    /// How the scope is typed.
    typed: &'a TypedDesc,
}

impl<'a> Binder<'a> {
    /// [ISO/IEC 9075-2:2023 §6.22 SR1, §6.24 SR1] `subject`, a reference
    /// whose scope is not empty, bound.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is of no reference type, or one of no scope.
    fn referenced(&self, subject: &ValueExpression, scope: Scope<'_>) -> Result<Referenced<'a>> {
        let (e, t) = self.value_expression(subject, scope)?;
        let DataType::Reference(r) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.22 SR1] a value of {t} is dereferenced, which is no reference")));
        };
        let Some(s) = &r.scope else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.22 SR1] {t} has no scope, so it is not dereferenced")));
        };
        let qn = QualifiedName { auth: s.schema.clone(), ident: s.name.clone() };
        let typed = self.typed_of(&qn).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.22 SR1] no typed scope table {qn}")))?;
        let of = self.structured_desc(&r.referenced.name)?;
        Ok(Referenced { value: (e, t.clone()), of, table: qn, typed })
    }

    /// How the table or view `qn` is typed, if it is a typed table or a
    /// referenceable view.
    fn typed_of(&self, qn: &QualifiedName) -> Option<&'a TypedDesc> {
        let catalog: &'a dyn crate::catalog::Catalog = &*self.db.catalog;
        catalog.table(qn).and_then(|t| t.typed.as_ref()).or_else(|| catalog.view(qn).and_then(|v| v.typed.as_ref()))
    }

    /// [ISO/IEC 9075-2:2023 §4.17.9, §11.32 SR22] The typed tables, or
    /// referenceable views, of the family of `top`: it and its proper
    /// subtables or subviews, each with how it is typed, the most specific
    /// first, so a row is found in the table of its own.
    fn family_members(&self, top: &QualifiedName) -> Vec<(QualifiedName, &'a TypedDesc)> {
        let catalog = &*self.db.catalog;
        let mut all: Vec<QualifiedName> = std::iter::once(top.clone()).chain(proper_subtables(catalog, top)).collect();
        all.sort_by_key(|t| std::cmp::Reverse(proper_supertables(catalog, t).len()));
        all.into_iter().filter_map(|t| self.typed_of(&t).map(|typed| (t, typed))).collect()
    }

    /// [ISO/IEC 9075-2:2023 §6.22] `subject` -> `attribute`: the column of
    /// the attribute's name of the row of the scope table the reference
    /// identifies, null if none.
    ///
    /// # Errors
    ///
    /// As `referenced`'s; SQLCODE -103 if the type has no such attribute
    /// [SR2].
    pub(in crate::binder) fn dereference(&self, subject: &ValueExpression, attribute: &str, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let r = self.referenced(subject, scope)?;
        let Some((_, a)) = r.of.attribute(attribute) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.22 SR2] {} has no attribute {attribute}", r.of.name)));
        };
        let result = a.data_type.clone();
        let column = ValueExpression::Column(column_of(SCOPED, attribute));
        let e = self.read_through(&r, (&r.table, false), (column, result.clone()))?;
        Ok((e, result))
    }

    /// [ISO/IEC 9075-2:2023 §6.23] `subject` -> `name` (`args`): the method
    /// invocation of DEREF(`subject`).
    ///
    /// # Errors
    ///
    /// As `reference_resolution`'s and `method_of`'s; SQLCODE -104 if the
    /// current privileges hold no SELECT on the table/method pair of the
    /// scope table and the subject routine [AR1].
    pub(in crate::binder) fn method_reference(
        &self,
        subject: &ValueExpression,
        name: &str,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let r = self.referenced(subject, scope)?;
        let (e, t) = self.resolved(&r)?;
        let DataType::Structured(st) = &t else { return Err(rule("[DESIGN] DEREF of no structured type")) };
        let of = st.name.clone();
        let invoked = self.method_of(((e, t), &of), (name, args), (false, scope))?;
        let subject_routine = match (&invoked.0, r.of.attribute(name), args.len()) {
            (Expr::Invoked(i), ..) => Some(i.specific.clone()),
            (_, Some((_, a)), 0) => Some(QualifiedName { auth: a.defined_by.auth.clone(), ident: a.observer.specific.clone() }),
            (_, Some((_, a)), _) => Some(QualifiedName { auth: a.defined_by.auth.clone(), ident: a.mutator.specific.clone() }),
            (_, None, _) => None,
        };
        // [AR1] SELECT on the table/method pair of SCOPE and SR.
        if let Some(sr) = subject_routine
            && !applicable(&*self.db.catalog, self.who(), &r.table).0.typed.methods.contains(&sr)
        {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §6.23 AR1] {} holds no SELECT on the table/method pair of {} and method {sr}",
                self.who().auth,
                r.table
            );
            return Err(SqlError::new(Code::Privilege, msg));
        }
        Ok(invoked)
    }

    /// [ISO/IEC 9075-2:2023 §6.24] DEREF(`subject`): the value of the
    /// referenced type of the row the reference identifies, of the
    /// table's type that has it, null if none.
    ///
    /// # Errors
    ///
    /// As `referenced`'s.
    pub(in crate::binder) fn reference_resolution(&self, subject: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let r = self.referenced(subject, scope)?;
        self.resolved(&r)
    }

    /// DEREF of the reference `r`, as `reference_resolution` gives it.
    fn resolved(&self, r: &Referenced<'a>) -> Result<(Expr, DataType)> {
        let result = r.of.data_type();
        let mut found: Option<Expr> = None;
        for (t, typed) in self.family_members(&r.table) {
            let desc = self.structured_desc(&udt_name_of(&typed.udt))?;
            let columns: Vec<ValueExpression> =
                desc.attributes.iter().map(|a| ValueExpression::Column(column_of(SCOPED, &a.name))).collect();
            let row = ValueExpression::Composite(Box::new(ast::Composite::Row(columns)), 0);
            let fields = desc.attributes.iter().map(|a| Field { name: a.name.clone(), data_type: a.data_type.clone(), unnamed: false });
            let row_type = DataType::row(fields.collect());
            let read = self.read_through(r, (&t, true), (row, row_type))?;
            let instance = call(Instanced { of: Arc::new(udt_name(desc)), result: result.clone() }, vec![read]);
            // A row is of one table alone: the first found, or this one.
            found = Some(match found {
                None => instance,
                Some(before) => Expr::Case(vec![(Cond::IsNull(before.clone(), true), before)], Box::new(instance)),
            });
        }
        let e = found.ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.24] scope table {} is no typed table", r.table)))?;
        Ok((e, result))
    }

    /// [ISO/IEC 9075-2:2023 §6.17, Feature S162] TREAT (`value` AS `target`)
    /// of a reference `value` to a reference type `target`: of the target
    /// type, the reference checked, as it runs, to identify a row of a
    /// table whose type is the target's referenced type or a subtype of it
    /// (`TreatReference`). The tables are the operand's scope's family, or
    /// with no scope each table of its referenced type or a subtype, each
    /// read as its owner, as no Access Rule asks a privilege of TREAT.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the target is no reference type, or its referenced
    /// type is not a subtype of the operand's [SR3].
    pub(in crate::binder) fn treat_reference(&self, value: (Expr, DataType), target: &ast::TypeName) -> Result<(Expr, DataType)> {
        let (DataType::Reference(vt), (DataType::Reference(dt), _)) = (&value.1, self.db.resolve_type(target, (self.schema, self.who()))?)
        else {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.17 SR1] TREAT of a reference is to a reference type".to_owned()));
        };
        let (operand, wanted) = (self.structured_desc(&vt.referenced.name)?, self.structured_desc(&dt.referenced.name)?);
        if wanted.name != operand.name && !wanted.supertypes.contains(&vt.referenced.name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.17 SR3] {} is no subtype of {}", wanted.name, operand.name)));
        }
        let (fitting, of_operand) = (self.family_below(wanted), self.family_below(operand));
        let members = match &vt.scope {
            Some(s) => self.family_members(&QualifiedName { auth: s.schema.clone(), ident: s.name.clone() }),
            None => self.typed_of_types(&of_operand),
        };
        let (mut fits, mut other) = (Vec::new(), Vec::new());
        let catalog = &*self.db.catalog;
        for (t, typed) in members {
            let looked = Looked { reference: &value, self_referencing: &typed.self_referencing, owner: Some(owner_of(catalog, &t.auth)) };
            let truth = ValueExpression::Value(ast::ValueSpecification::Literal(ast::Literal::Boolean(Some(true))));
            let found = self.looked_up(&looked, (&t, true), (truth, DataType::Boolean))?;
            let into = if fitting.iter().any(|u| u.schema == typed.udt.auth && u.name == typed.udt.ident) { &mut fits } else { &mut other };
            into.push(found);
        }
        let result = DataType::Reference(dt);
        let f = crate::functions::TreatReference { result: result.clone(), same: wanted.name == operand.name };
        Ok((call(f, vec![value.0, any_of(fits), any_of(other)]), result))
    }

    /// The typed tables and referenceable views whose type is one of
    /// `types`, each with how it is typed.
    fn typed_of_types(&self, types: &[crate::value::types::UdtName]) -> Vec<(QualifiedName, &'a TypedDesc)> {
        let catalog: &'a dyn crate::catalog::Catalog = &*self.db.catalog;
        let tables = catalog.tables().into_iter().filter_map(|t| t.typed.as_ref().map(|d| (t.name.clone(), d)));
        let views = catalog.views().into_iter().filter_map(|v| v.typed.as_ref().map(|d| (v.name.clone(), d)));
        let of = |d: &TypedDesc| types.iter().any(|u| u.schema == d.udt.auth && u.name == d.udt.ident);
        tables.chain(views).filter(|(_, d)| of(d)).collect()
    }

    /// The invocation of `SELECT value FROM [ONLY] table WHERE
    /// selfref = r`, over the row of the reference `r`, of type `result`:
    /// null if no row is identified.
    fn read_through(
        &self,
        r: &Referenced<'a>,
        (table, only): (&QualifiedName, bool),
        (value, result): (ValueExpression, DataType),
    ) -> Result<Expr> {
        let looked = Looked { reference: &r.value, self_referencing: &r.typed.self_referencing, owner: None };
        self.looked_up(&looked, (table, only), (value, result))
    }

    /// The invocation of `SELECT value FROM [ONLY] table WHERE
    /// selfref = r` of `looked`, of type `result`: null if no row is
    /// identified.
    fn looked_up(
        &self,
        looked: &Looked<'_, 'a>,
        (table, only): (&QualifiedName, bool),
        (value, result): (ValueExpression, DataType),
    ) -> Result<Expr> {
        let item = reference_item(&looked.reference.1);
        let names = item.row_names();
        let cx = match looked.owner {
            Some(owner) => super::super::Context { auth: owner, role: None, ..self.context() },
            None => self.context(),
        };
        let binder = Binder::new(cx, None, (vec![item], names, vec![(0, 0)]), BTreeSet::new());
        let key = SearchCondition::Predicate(
            Box::new(Predicate::Comparison(
                vec![ValueExpression::Column(column_of(SCOPED, looked.self_referencing))],
                CompOp::Eq,
                vec![ValueExpression::Column(column_of(REFERENCE, REFERENCE))],
            )),
            0,
        );
        let from = TableReference::Table(NamedTable {
            table: TableName { auth: Some(table.auth.clone()), ident: table.ident.clone(), at: 0 },
            only,
            correlation: Some(SCOPED.to_owned()),
            columns: None,
            system_time: None,
        });
        let table_expression =
            TableExpression { from: vec![from], where_clause: Some(key), group_by: None, having: None, window: Vec::new() };
        let select_list = SelectList::Values(vec![SelectSublist::Derived(SelectItem { value, alias: None, at: 0 })]);
        let query = QueryExpression::Spec(Box::new(QuerySpecification { distinct: false, select_list, table_expression }));
        let (body, _) = binder.subquery(&ast::Subquery { query, at: 0 }, Scope::Rows(ONE_ROW), Use::Rows)?;
        let invocation = Invocation {
            name: QualifiedName { auth: table.auth.clone(), ident: DEREFERENCE.to_owned() },
            specific: QualifiedName { auth: table.auth.clone(), ident: DEREFERENCE.to_owned() },
            args: vec![looked.reference.0.clone()],
            params: vec![(REFERENCE.to_owned(), looked.reference.1.clone())],
            body: Some(body),
            program: None,
            modifies: false,
            result,
            null_call: true,
            deterministic: false,
            overrides: None,
        };
        Ok(Expr::Invoked(Arc::new(invocation)))
    }
}

/// True if one of `found`, each true or null, is true; null otherwise.
fn any_of(found: Vec<Expr>) -> Expr {
    let mut all = found.into_iter();
    let first = all.next().unwrap_or(Expr::Const(crate::value::Value::Null));
    all.fold(first, |before, next| Expr::Case(vec![(Cond::IsNull(before.clone(), true), before)], Box::new(next)))
}

/// The column `column` of the table whose correlation name is `table`.
fn column_of(table: &str, column: &str) -> ColumnSpecification {
    ColumnSpecification {
        qualifier: Some(TableName { auth: None, ident: table.to_owned(), at: 0 }),
        column: column.to_owned(),
        catalog: None,
        at: 0,
    }
}

/// The `UdtName` of the type `qn` names.
fn udt_name_of(qn: &QualifiedName) -> crate::value::types::UdtName {
    crate::value::types::UdtName::new(&qn.auth, &qn.ident)
}

/// The row of the reference, of type `t`: a table of no schema whose one
/// column and correlation name are `REFERENCE`, its value the outer value
/// of the dereference's body.
fn reference_item(t: &DataType) -> FromItem {
    let column = ColumnDesc {
        name: REFERENCE.to_owned(),
        data_type: t.clone(),
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let name = QualifiedName { auth: String::new(), ident: REFERENCE.to_owned() };
    let table = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: vec![column],
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let source = Source::Derived(LogicalPlan::Values { rows: Vec::new(), types: vec![t.clone()] });
    FromItem { correlation: Some(REFERENCE.to_owned()), ..FromItem::alone(name, 0, (table, source)) }
}
