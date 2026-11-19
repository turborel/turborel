//! \[EXTENSION\] A plug-in's procedure (RULES.md D169), a `Procedure`.
//! While its CALL runs, the program may send SQL frames, each a statement
//! or query the procedure runs in the CALL's session and transaction with
//! its user's privileges (`ProcedureContext`), answered by SQL_DONE or
//! ERROR, before it sends its RESULT. A procedure whose manifest does not
//! say MODIFIES SQL DATA may only read.

use std::cell::RefCell;

use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::plugin::manifest::Declared;
use crate::plugin::payload::PROCEDURE_CALL;
use crate::plugin::pool::CallContext;
use crate::plugin::values::put_values;
use crate::value::{DataType, Value};

use super::{Link, Resolve, answer, resolved};

/// [ISO/IEC 9075-2:2023 §24.1] SQLSTATE 2F002, modifying SQL-data not
/// permitted.
const MODIFYING_PROHIBITED: SqlState = SqlState::of_bytes(*b"2F002");

/// A plug-in's procedure.
#[derive(Debug)]
pub struct PluginProcedure {
    link: Link,
    parameters: Vec<DataType>,
    modifies: bool,
}

impl PluginProcedure {
    /// The procedure `d` declares.
    ///
    /// # Errors
    ///
    /// A type `resolve` refuses.
    pub fn new(d: &Declared, link: Link, resolve: Resolve<'_>) -> Result<PluginProcedure> {
        Ok(PluginProcedure { link, parameters: resolved(&d.parameters, resolve)?, modifies: d.modifies })
    }
}

impl Procedure for PluginProcedure {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == self.parameters.len()).then(|| self.parameters.clone())
    }
    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
        let mut payload = Vec::new();
        put_values(&mut payload, args)?;
        let cx = RefCell::new(cx);
        let modifies = self.modifies;
        let mut sql = |rows: bool, text: &str, given: &[Value]| -> Result<Vec<Vec<Value>>> {
            let mut cx = cx.borrow_mut();
            if rows {
                return cx.rows(text, given);
            }
            if !modifies {
                let msg = format!("[EXTENSION] {} does not say MODIFIES SQL DATA, so it runs queries alone, not {text}", self.link.named());
                return Err(SqlError::new(Code::SyntaxRule, msg).with_state(MODIFYING_PROHIBITED));
            }
            cx.run(text, given).map(|()| Vec::new())
        };
        let got = self.link.call(PROCEDURE_CALL, payload, &mut CallContext { interrupt: None, sql: Some(&mut sql) })?;
        answer(&got, |_| Ok(()))
    }
}
