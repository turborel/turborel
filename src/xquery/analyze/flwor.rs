//! The expressions that bind variables, compiled [XQuery §3.8, §3.11,
//! §3.12.2]: each variable a slot of the frame, in scope in what follows
//! its binding and not in the expression it is bound to [XQuery §3.8.1].

use super::super::ast::{
    CaseClause, Direction, EmptyOrder, Flwor, FlworClause, OrderSpec, QName as Lexical, Quantified, Quantifier, Typeswitch,
};
use super::super::collation::Collator;
use super::super::error::{ErrorCode, error};
use super::super::plan::{Clause, FlworOp, Op, OrderKey, QuantifiedOp, TypeswitchOp};
use super::Analysis;
use super::scope::{Unprefixed, Var};
use crate::error::Result;

impl Analysis<'_> {
    /// `name` bound to a new slot of the frame for what follows: the slot.
    fn bind_new(&mut self, name: &Lexical) -> Result<usize> {
        let expanded = self.scope.name(name, Unprefixed::None)?;
        let slot = self.scope.slot();
        self.scope.bind(expanded, Var::Local(slot));
        Ok(slot)
    }

    /// A FLWOR expression [XQuery §3.8].
    pub(super) fn flwor(&mut self, f: &Flwor) -> Result<Op> {
        let mut clauses = Vec::new();
        let mut bound = 0_usize;
        let compiled = (|| -> Result<FlworOp> {
            for clause in &f.clauses {
                bound = bound.saturating_add(self.clause(clause, &mut clauses)?);
            }
            let condition = f.condition.as_ref().map(|c| self.expr(c)).transpose()?;
            let order = match &f.order_by {
                Some(order_by) => order_by.specs.iter().map(|s| self.order_spec(s)).collect::<Result<Vec<_>>>()?,
                None => Vec::new(),
            };
            let result = self.expr(&f.result)?;
            Ok(FlworOp { clauses: std::mem::take(&mut clauses), condition, order, result })
        })();
        self.scope.unbind(bound);
        compiled.map(|op| Op::Flwor(Box::new(op)))
    }

    /// The variables of `clause` compiled into `out`, each bound after its
    /// expression [XQuery §3.8.1]: how many it bound.
    fn clause(&mut self, clause: &FlworClause, out: &mut Vec<Clause>) -> Result<usize> {
        let mut bound = 0_usize;
        match clause {
            FlworClause::For(bindings) => {
                for b in bindings {
                    let input = self.expr(&b.input)?;
                    let declared = b.declared.as_ref().map(|t| self.sequence_type(t)).transpose()?;
                    let slot = self.bind_new(&b.var)?;
                    let position = b.position.as_ref().map(|p| self.bind_new(p)).transpose()?;
                    bound = bound.saturating_add(if position.is_some() { 2 } else { 1 });
                    out.push(Clause::For(slot, position, declared, input));
                }
            }
            FlworClause::Let(bindings) => {
                for b in bindings {
                    let value = self.expr(&b.value)?;
                    let declared = b.declared.as_ref().map(|t| self.sequence_type(t)).transpose()?;
                    let slot = self.bind_new(&b.var)?;
                    bound = bound.saturating_add(1);
                    out.push(Clause::Let(slot, declared, value));
                }
            }
        }
        Ok(bound)
    }

    /// An order spec [XQuery §3.8.3]: its collation one statically known
    /// \[err:XQST0076\], else the default; where the empty sequence orders
    /// as written, else by the default order for empty sequences.
    fn order_spec(&mut self, spec: &OrderSpec) -> Result<OrderKey> {
        let key = self.expr(&spec.key)?;
        let collation = match &spec.collation {
            Some(uri) => {
                let Some(found) = Collator::named(uri, self.env.collations) else {
                    return Err(error(ErrorCode::XQST0076, &format!("no statically known collation is '{uri}' [XQuery §3.8.3]")));
                };
                (found, true)
            }
            None => (self.settings.collation.clone(), false),
        };
        let empty_greatest = spec.empty.map_or(self.empty_greatest, |e| e == EmptyOrder::Greatest);
        Ok(OrderKey { key, descending: spec.direction == Some(Direction::Descending), empty_greatest, collation })
    }

    /// A quantified expression [XQuery §3.11].
    pub(super) fn quantified(&mut self, q: &Quantified) -> Result<Op> {
        let mut bindings = Vec::new();
        let mut bound = 0_usize;
        let compiled = (|| -> Result<Op> {
            for b in &q.bindings {
                let input = self.expr(&b.input)?;
                let declared = b.declared.as_ref().map(|t| self.sequence_type(t)).transpose()?;
                let slot = self.bind_new(&b.var)?;
                bound = bound.saturating_add(1);
                bindings.push((slot, declared, input));
            }
            let test = self.expr(&q.test)?;
            let every = q.quantifier == Quantifier::Every;
            Ok(Op::Quantified(Box::new(QuantifiedOp { every, bindings: std::mem::take(&mut bindings), test })))
        })();
        self.scope.unbind(bound);
        compiled
    }

    /// A typeswitch expression [XQuery §3.12.2]: each clause's variable in
    /// scope in its clause alone.
    pub(super) fn typeswitch(&mut self, t: &Typeswitch) -> Result<Op> {
        let operand = self.expr(&t.operand)?;
        let cases = t.cases.iter().map(|c| self.case(c)).collect::<Result<Vec<_>>>()?;
        let default = self.scoped(t.default_var.as_ref(), |a| a.expr(&t.default))?;
        Ok(Op::Typeswitch(Box::new(TypeswitchOp { operand, cases, default })))
    }

    /// A case clause of a typeswitch.
    fn case(&mut self, c: &CaseClause) -> Result<(Option<usize>, super::super::types::SeqType, Op)> {
        let matches = self.sequence_type(&c.matches)?;
        let (slot, result) = self.scoped(c.var.as_ref(), |a| a.expr(&c.result))?;
        Ok((slot, matches, result))
    }

    /// `compile` with `var`, if any, bound to a new slot within it.
    fn scoped(&mut self, var: Option<&Lexical>, compile: impl FnOnce(&mut Self) -> Result<Op>) -> Result<(Option<usize>, Op)> {
        let slot = var.map(|v| self.bind_new(v)).transpose()?;
        let compiled = compile(self);
        if slot.is_some() {
            self.scope.unbind(1);
        }
        Ok((slot, compiled?))
    }
}
