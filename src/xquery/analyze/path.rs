//! Paths compiled [XQuery §3.2]: a leading `/` the root of the context
//! node's tree, each `//` a `descendant-or-self::node()` step [XQuery
//! §3.2.4], each abbreviated step its axis, and each name test's QName
//! expanded [XQuery §3.2.1.2].

use super::super::ast::{ForwardAxis, KindTest, NameTest, NodeTest, Path, ReverseAxis, Root, Separator, Step, StepExpr};
use super::super::error::ErrorCode;
use super::super::plan::{Axis, Namespace, Op, PathOp, PathStart, StepOp, StepTest};
use super::super::types::NodeKind;
use super::Analysis;
use super::scope::Unprefixed;
use crate::error::Result;
use crate::xml::Kind;

impl Analysis<'_> {
    /// The path `path` compiled [XQuery §3.2].
    pub(super) fn path(&mut self, path: &Path) -> Result<Op> {
        let mut steps = Vec::new();
        let start = match path.root {
            Root::Relative => PathStart::Context,
            Root::Slash => PathStart::Root,
            Root::DoubleSlash => {
                steps.push(descendant_or_self());
                PathStart::Root
            }
        };
        if let Some(first) = &path.first {
            steps.push(self.step(first)?);
        }
        for (separator, step) in &path.rest {
            if *separator == Separator::DoubleSlash {
                steps.push(descendant_or_self());
            }
            steps.push(self.step(step)?);
        }
        Ok(Op::Path(Box::new(PathOp { start, steps })))
    }

    /// A step compiled [XQuery §3.2.1].
    fn step(&mut self, step: &StepExpr) -> Result<StepOp> {
        match step {
            StepExpr::Filter(primary, predicates) => Ok(StepOp::Filter(self.expr(primary)?, self.all(predicates)?)),
            StepExpr::Axis(a) => {
                let (axis, test) = match &a.step {
                    Step::Forward(axis, test) => (forward(*axis), test),
                    Step::Reverse(axis, test) => (reverse(*axis), test),
                    // [XQuery §3.2.4] child, or attribute of an attribute test.
                    Step::Abbreviated(test) => {
                        let attribute = matches!(test, NodeTest::Kind(KindTest::Attribute(_) | KindTest::SchemaAttribute(_)));
                        (if attribute { Axis::Attribute } else { Axis::Child }, test)
                    }
                    Step::Attribute(test) => (Axis::Attribute, test),
                    Step::Parent => return Ok(StepOp::Axis(Axis::Parent, StepTest::Kind(NodeKind::Any), self.all(&a.predicates)?)),
                };
                let test = self.node_test(test, axis.principal())?;
                Ok(StepOp::Axis(axis, test, self.all(&a.predicates)?))
            }
        }
    }

    /// A node test resolved [XQuery §3.2.1.2]: a name test's prefix bound
    /// \[err:XPST0008\], an unprefixed name of an axis whose principal node
    /// kind is element in the default element/type namespace, else in
    /// none.
    fn node_test(&self, test: &NodeTest, principal: Kind) -> Result<StepTest> {
        let unprefixed = if principal == Kind::Element { Unprefixed::Element } else { Unprefixed::None };
        Ok(match test {
            NodeTest::Kind(kind) => StepTest::Kind(self.kind_test(kind)?),
            NodeTest::Name(NameTest::Any) => StepTest::Name(Namespace::Any, None),
            NodeTest::Name(NameTest::Local(local)) => StepTest::Name(Namespace::Any, Some(local.as_str().into())),
            NodeTest::Name(NameTest::Name(name)) => {
                let expanded = self.scope.expand(name, unprefixed, ErrorCode::XPST0008)?;
                StepTest::Name(Namespace::of(expanded.uri), Some(expanded.local))
            }
            NodeTest::Name(NameTest::Prefix(prefix)) => {
                let name = super::super::ast::QName { prefix: Some(prefix.clone()), local: String::new() };
                let expanded = self.scope.expand(&name, unprefixed, ErrorCode::XPST0008)?;
                StepTest::Name(Namespace::of(expanded.uri), None)
            }
        })
    }
}

/// `descendant-or-self::node()`, what `//` stands for [XQuery §3.2.4].
fn descendant_or_self() -> StepOp {
    StepOp::Axis(Axis::DescendantOrSelf, StepTest::Kind(NodeKind::Any), Vec::new())
}

/// The axis of a forward step.
fn forward(axis: ForwardAxis) -> Axis {
    match axis {
        ForwardAxis::Child => Axis::Child,
        ForwardAxis::Descendant => Axis::Descendant,
        ForwardAxis::Attribute => Axis::Attribute,
        ForwardAxis::SelfAxis => Axis::SelfNode,
        ForwardAxis::DescendantOrSelf => Axis::DescendantOrSelf,
        ForwardAxis::FollowingSibling => Axis::FollowingSibling,
        ForwardAxis::Following => Axis::Following,
    }
}

/// The axis of a reverse step.
fn reverse(axis: ReverseAxis) -> Axis {
    match axis {
        ReverseAxis::Parent => Axis::Parent,
        ReverseAxis::Ancestor => Axis::Ancestor,
        ReverseAxis::PrecedingSibling => Axis::PrecedingSibling,
        ReverseAxis::Preceding => Axis::Preceding,
        ReverseAxis::AncestorOrSelf => Axis::AncestorOrSelf,
    }
}
