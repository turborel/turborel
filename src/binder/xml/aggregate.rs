//! Binding XMLAGG [ISO/IEC 9075-14:2006 draft §11.2], an `<aggregate
//! function>` of an XML value ordered as ARRAY_AGG's value is.

use std::sync::Arc;

use super::super::{Binder, Scope};
use super::publish::IMPLICIT_PUBLISHING;
use crate::ast::{Returning, SetFunctionSpecification};
use crate::error::Result;
use crate::functions::{AggregateRef, XmlAggregate};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [§11.2] XMLAGG ( XVE \[ORDER BY K1, ...\] \[RETURNING ...\] ): the
    /// aggregate of the row ( XVE, K1, ... ), RETURNING CONTENT by default
    /// \[SR1\], implementation-dependent, as the publishing operators are.
    pub(in crate::binder) fn xml_aggregate(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let content = sf.returning.unwrap_or(IMPLICIT_PUBLISHING) == Returning::Content;
        self.ordered_aggregate(sf, scope, |orders, collations| AggregateRef(Arc::new(XmlAggregate { orders, collations, content })))
    }
}
