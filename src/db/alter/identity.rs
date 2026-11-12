//! [ISO/IEC 9075-2:2023 §11.20 to §11.22] ALTER COLUMN of an identity
//! column or a generated column: SET GENERATED and the options of its
//! sequence generator, DROP IDENTITY and DROP EXPRESSION. Each changes the
//! column's descriptor alone, and no row.

use crate::ast::AlterColumnAction;
use crate::catalog::{ColumnDesc, Generated, QualifiedName};
use crate::db::State;
use crate::db::sequences::Epochs;
use crate::error::{Code, Result, SqlError};

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

impl State {
    /// Column `c` of table `qn` altered as `action` says, an action of
    /// §11.20 to §11.22, written at `at`: a restart's epoch from `epochs`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column is not an identity column [§11.12 SR7],
    /// or not a generated column [§11.12 SR8], or the options break §9.37's
    /// Syntax Rules.
    pub(in crate::db) fn alter_generation(
        &self,
        (qn, c): (&QualifiedName, &mut ColumnDesc),
        (action, at): (&AlterColumnAction, usize),
        epochs: &mut Epochs,
    ) -> Result<()> {
        let no_identity = |name: &str| rule(format!("[ISO/IEC 9075-2:2023 §11.12 SR7] column {name} of {qn} is not an identity column"));
        match action {
            AlterColumnAction::Identity { always, options } => {
                let Some(Generated::Identity(identity)) = &mut c.generated else { return Err(no_identity(&c.name)) };
                // [§11.20 SR1, GR1] the options, as §9.37 alters a generator.
                identity.sequence = self.altered(identity.sequence.clone(), options, (at, epochs))?;
                // [§11.20 GR2] ALWAYS, or BY DEFAULT.
                if let Some(always) = always {
                    identity.always = *always;
                }
            }
            // [§11.21 GR2] no identity column; [GR3] its generator destroyed,
            // its counter dropped as the database next opens (RULES.md D62).
            AlterColumnAction::DropIdentity => {
                if c.identity().is_none() {
                    return Err(no_identity(&c.name));
                }
                c.generated = None;
            }
            // [§11.22 GR2] a base column, "NEVER" generated, its values kept.
            AlterColumnAction::DropExpression => {
                if c.generation().is_none() {
                    return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.12 SR8] column {} of {qn} is not a generated column", c.name)));
                }
                c.generated = None;
            }
            AlterColumnAction::SetDefault(_) | AlterColumnAction::DropDefault => {
                return Err(crate::db::api("[DESIGN] a default is altered by db/defaults.rs"));
            }
        }
        Ok(())
    }
}
