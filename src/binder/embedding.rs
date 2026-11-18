//! \[EXTENSION\] A call of EMBED, VECTOR_EMBEDDING or EMBED_VERSION naming
//! an embedding model of the catalog (the vector design's §12.1, §16):
//! its first argument a character string literal naming the model, as
//! `'M'` or `'U.M'`, or a column reference naming one that is no column.
//! It is bound to the model, fixed as the statement is bound
//! (`vector/embed/model.rs`): EMBED of type VECTOR of the model's shape
//! (R12.1). The current privileges need EXECUTE on it. A call naming no
//! model of the catalog calls the plug-in its argument names
//! (`vector/embed/routines.rs`); a literal naming neither is refused, so
//! what named a model dropped is found by binding it again.

use std::sync::Arc;

use super::{Binder, Scope, qualify};
use crate::ast::{Literal, ValueExpression, ValueSpecification};
use crate::catalog::{QualifiedName, owner_of};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::Expr;
use crate::value::DataType;
use crate::vector::embed::{BoundEmbed, BoundVersion, EMBED_NAMES, EMBED_VERSION};

impl Binder<'_> {
    /// The model of the catalog `first`, a call's first argument, names.
    fn model_argument(&self, first: &ValueExpression) -> Option<QualifiedName> {
        let named = if let ValueExpression::Value(ValueSpecification::Literal(Literal::Character(text))) = first {
            crate::db::model_named(text)?
        } else if let ValueExpression::Column(c) = first {
            // A column of a table of FROM, or a name of more than two parts, is none.
            let column = match &c.qualifier {
                None => self.visible_has(&c.column),
                Some(q) => self.items.iter().any(|it| it.designated_by(q, self.schema)),
            };
            let qualifier = c.qualifier.as_ref().filter(|q| q.auth.is_none());
            if column || c.catalog.is_some() || (c.qualifier.is_some() && qualifier.is_none()) {
                return None;
            }
            crate::ast::TableName { auth: qualifier.map(|q| q.ident.clone()), ident: c.column.clone(), at: 0 }
        } else {
            return None;
        };
        let qn = qualify(&named, self.schema);
        self.db.catalog.embeddings().model(&qn).map(|_| qn)
    }

    /// EMBED, VECTOR_EMBEDDING or EMBED_VERSION `name` of `args`, bound to
    /// the model of the catalog its first argument names; none if it is
    /// not one of them, or names none.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the current privileges hold no EXECUTE on the
    /// model; -103 if its plug-in is not registered, or its other
    /// arguments are not texts.
    pub(super) fn catalog_embed(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Result<Option<(Expr, DataType)>> {
        let embed = EMBED_NAMES.iter().find(|n| **n == name);
        if embed.is_none() && name != EMBED_VERSION {
            return Ok(None);
        }
        let Some((first, rest)) = args.split_first() else { return Ok(None) };
        let Some(qn) = self.model_argument(first) else {
            // A literal names a model of the catalog, else a plug-in.
            if let ValueExpression::Value(ValueSpecification::Literal(Literal::Character(text))) = first
                && self.db.embedders.named(text)?.is_none()
            {
                return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] VEC-0030 {name} names no model {text}")));
            }
            return Ok(None);
        };
        let desc = self.db.model_desc(&qn)?;
        if !desc.execute_of(self.who(), owner_of(&*self.db.catalog, &qn.auth)).0 {
            return Err(SqlError::new(Code::Privilege, format!("[EXTENSION] {} holds no EXECUTE on embedding model {qn}", self.auth)));
        }
        let model = self.db.embedders.bind(&desc)?;
        let f: Arc<dyn ScalarFunction> = match embed {
            Some(called) => Arc::new(BoundEmbed { name: called, model }),
            None => Arc::new(BoundVersion { model }),
        };
        let mut exprs = Vec::with_capacity(rest.len());
        let mut types = Vec::with_capacity(rest.len());
        for a in rest {
            let (e, t) = self.value_expression(a, scope)?;
            exprs.push(e);
            types.push(t);
        }
        let t = f.return_type(&types)?;
        Ok(Some((Expr::Call(ScalarRef(f), exprs), t)))
    }
}
