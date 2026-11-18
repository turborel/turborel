//! \[EXTENSION\] A routine written in Rust (`functions/builtin.rs`)
//! invoked as a function, by its name, unqualified, in a value
//! expression: a table function that is invoked so too
//! (`TableFunction::scalar`), its arguments and the queries it reads bound
//! as `TABLE(f(args))` binds them; or a procedure that is a function too
//! (`Procedure::returns`), which possibly modifies SQL-data, so it is
//! invoked only where a function that does may be [ISO/IEC 9075-2:2023
//! §7.17 SR25], and runs where the SQL-session runs its statements
//! (RULES.md D-SPATIAL-ROUTINES). They are how SQL/MM Spatial's topology
//! functions are made [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3].

use std::sync::Arc;

use super::builtin_table::{Parameters, known};
use super::{Binder, Purpose, Scope, rule};
use crate::ast::ValueExpression;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::functions::builtin::{Procedure, TableCall, TableFunction, ValueCall};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Expr, Invocation};
use crate::parser::Parser;
use crate::value::{DataType, Value};

impl Binder<'_> {
    /// The invocation of `name` with `args` in `scope` if it names a
    /// routine written in Rust invoked as a function; none else.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it takes no such count of arguments, an argument is
    /// not assignable to its parameter, or what contains it may not invoke
    /// a function that possibly modifies SQL-data; the function's, naming
    /// what it reads; a query's.
    pub(super) fn builtin_function(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Result<Option<(Expr, DataType)>> {
        if let Some(f) = self.db.functions.table(name).filter(|f| f.scalar()) {
            return self.table_value(&f, args, scope).map(Some);
        }
        let Some(p) = self.db.functions.procedure(name) else { return Ok(None) };
        let Some(result) = p.returns(args.len()) else { return Ok(None) };
        self.procedure_value(p, result, args, scope).map(Some)
    }

    /// The value of table function `f` of `args`.
    fn table_value(&self, f: &Arc<dyn TableFunction>, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let types = f
            .parameters(args.len())
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] function {} takes no {} arguments", f.name(), args.len())))?;
        let (mut exprs, _) = self.arguments(&Parameters(types), args, scope)?;
        let values: Vec<Option<Value>> = exprs.iter().map(known).collect();
        let refs: Vec<Option<&Value>> = values.iter().map(Option::as_ref).collect();
        for query in f.reads(&refs)? {
            let read = Parser::new(&format!("MULTISET ({query})"))?.generation_expression()?;
            exprs.push(self.value_expression(&read, scope).map_err(|e| f.read_failed(e))?.0);
        }
        let call = ValueCall(TableCall { function: Arc::clone(f), arguments: args.len() });
        let t = call.return_type(&[])?;
        Ok((Expr::Call(ScalarRef(Arc::new(call)), exprs), t))
    }

    /// [ISO/IEC 9075-2:2023 §9.18] The invocation of procedure `p` as a
    /// function of `args`, of result type `result`.
    fn procedure_value(
        &self,
        p: Arc<dyn Procedure>,
        result: DataType,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let name = p.name().to_owned();
        if !(self.purpose == Purpose::Query && self.hosting.get()) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.17 SR25, §11.49 SR11] function {name} possibly modifies SQL-data, so it is not invoked here"
            )));
        }
        let types = p.parameters(args.len()).unwrap_or_default();
        let (exprs, _) = self.arguments(&Parameters(types.clone()), args, scope)?;
        let qualified = QualifiedName { auth: String::new(), ident: name };
        let params = (1_usize..).zip(types).map(|(k, t)| (format!("P{k}"), t)).collect();
        let invocation = Invocation {
            name: qualified.clone(),
            specific: qualified,
            args: exprs,
            params,
            body: None,
            program: None,
            modifies: true,
            result: result.clone(),
            null_call: false,
            deterministic: false,
            overrides: None,
            rust: Some(p),
        };
        Ok((Expr::Invoked(Arc::new(invocation)), result))
    }
}
