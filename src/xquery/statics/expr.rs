//! The expressions typed [XQueryFS §4], each as its normalization into the
//! Core is [XQueryFS §3.2.2]: primaries, paths, sequences, arithmetic and
//! comparisons, logic, FLWOR, quantified, conditional and typeswitch
//! expressions, and the expressions on sequence types; function calls in
//! `statics/functions.rs`, constructors in `statics/constructors.rs`.

use std::sync::Arc;

use super::super::ast::Comparator;
use super::super::plan::{Arith, Callee, Clause, CompareOp, FlworOp, Op, PathOp, PathStart, QuantifiedOp, StepOp, TypeswitchOp};
use super::axes::step;
use super::functions::{DATA, LAST, Pick, calls, is_numeric_literal, is_one, pick};
use super::operators::{arithmetic, binary, comparable, convert, is_equality};
use super::sequence::{any_document, node_items, normalized};
use super::subtype::subtype;
use super::types::{Atom, Item, Occ, Type, factored, is_empty, prime, quantifier};
use super::values::{boolean, data};
use super::{Frame, Statics, type_error};
use crate::error::Result;
use crate::xml::AtomicType;

impl Statics<'_> {
    /// [XQueryFS §3.2.3] `statEnv |- op : Type`: the static type of `op`
    /// in `frame`; [XQuery §2.3.1] err:XPST0005 if it is the empty type,
    /// but of `()`, of fn:data of `()`, and of a call of a function whose
    /// type is the empty type [XQueryFS §4].
    pub(super) fn expr(&mut self, op: &Op, frame: &mut Frame) -> Result<Type> {
        self.depth = self.depth.saturating_add(1);
        let typed = if self.depth > super::super::MAX_DEPTH {
            Err(super::beyond("the static typing nests deeper than turborel's limit"))
        } else {
            self.typed(op, frame)
        };
        self.depth = self.depth.saturating_sub(1);
        let t = typed?;
        if !self.exempt(op) && is_empty(&t)? {
            return Err(empty(op));
        }
        Ok(t)
    }

    /// The static type of `op`, by the rule of its kind.
    ///
    /// \[DESIGN\] RULES.md R-CF-6: each kind is typed by a function of its
    /// own, so the frame the nesting of expressions recurs through holds
    /// none of their locals.
    fn typed(&mut self, op: &Op, frame: &mut Frame) -> Result<Type> {
        match op {
            Op::Empty => Ok(Type::Empty),
            Op::Atomic(a) => Ok(Type::atomic(a.kind)),
            Op::Sequence(ops) => self.all(ops, frame).map(Type::seq),
            Op::Local(slot) => Ok(frame.slots.get(*slot).cloned().flatten().unwrap_or(Type::None)),
            Op::Global(at) => Ok(self.globals.get(*at).cloned().unwrap_or(Type::None)),
            Op::ContextItem => frame.focus.clone().ok_or_else(|| super::no_focus(".")),
            Op::Path(p) => self.path(p, frame),
            Op::Filter(primary, predicates) => self.filter(primary, predicates, frame),
            Op::Range(operands) => self.range(operands, frame),
            Op::Arithmetic(first, rest) => self.arithmetic(first, rest, frame),
            Op::Unary(_, operand) => self.unary(operand, frame),
            Op::Union(ops) => self.union(ops, frame),
            Op::IntersectExcept(first, rest) => self.intersect_except(first, rest, frame),
            Op::Compare(c) => self.compare(c, frame),
            Op::And(ops) | Op::Or(ops) => self.logical(ops, frame),
            Op::If(parts) => self.conditional(parts, frame),
            Op::Flwor(f) => self.flwor(f, frame),
            Op::Quantified(q) => self.quantified(q, frame),
            Op::Typeswitch(t) => self.typeswitch(t, frame),
            Op::InstanceOf(operand, _) => self.expr(operand, frame).map(|_| Type::boolean()),
            Op::Treat(operand, t) => self.expr(operand, frame).map(|_| normalized(t)),
            Op::Castable(operand, target) => self.cast_operand(operand, target.optional, frame).map(|()| Type::boolean()),
            Op::Cast(operand, target) => self.cast(operand, target, frame),
            Op::Call(call) => self.call(call, frame),
            Op::Validate(_, operand) => self.validate(operand, frame),
            Op::Element(e) => self.direct(e, frame),
            Op::ComputedElement(e) => self.computed_element(e, frame),
            Op::Attribute(name, content) => self.attribute(name, content.as_deref(), frame),
            Op::Document(content) => self.document(content, frame),
            Op::Text(content) => self.expr(content, frame).map(|_| Type::occur(Type::Item(Item::Text), Occ::Optional)),
            Op::Comment(content) => self.expr(content, frame).map(|_| Type::Item(Item::Comment)),
            Op::DirectComment(_) => Ok(Type::Item(Item::Comment)),
            Op::Pi(pi) => self.pi(pi, frame),
        }
    }

    /// A filter expression [XQueryFS §4.3.2]: its primary filtered.
    fn filter(&mut self, primary: &Op, predicates: &[Op], frame: &mut Frame) -> Result<Type> {
        let t = self.expr(primary, frame)?;
        self.predicates(t, predicates, frame)
    }

    /// [XQueryFS §4.3.1, §7.1.11] `to`, `fs:to` of two optional integers
    /// converted as a function's arguments are: integers.
    fn range(&mut self, operands: &[Op; 2], frame: &mut Frame) -> Result<Type> {
        let integer = Type::occur(Type::atomic(AtomicType::Integer), Occ::Optional);
        for operand in operands {
            let t = self.expr(operand, frame)?;
            let converted = super::values::convert_untyped(&data(&t)?, &Type::atomic(AtomicType::Integer))?;
            if !super::subtype::can_promote(&converted, &integer)? {
                return Err(type_error(&format!("an operand of `to` is of {t}, not of xs:integer? [XQueryFS §7.1.11]")));
            }
        }
        Ok(Type::occur(Type::atomic(AtomicType::Integer), Occ::Star))
    }

    /// [XQueryFS §4.4] A unary operator: of a number, its type.
    fn unary(&mut self, operand: &Op, frame: &mut Frame) -> Result<Type> {
        let t = self.operand(operand, frame)?;
        binary(&t, &Type::atomic(AtomicType::Integer), "a unary operator", &unary)
    }

    /// [XQueryFS §4.6] `and` and `or`: of each operand's effective boolean
    /// value, `xs:boolean`.
    fn logical(&mut self, ops: &[Op], frame: &mut Frame) -> Result<Type> {
        for o in ops {
            let t = self.expr(o, frame)?;
            boolean(&t)?;
        }
        Ok(Type::boolean())
    }

    /// [XQueryFS §4.10] `if`: of its test's effective boolean value, the
    /// choice of its branches' types.
    fn conditional(&mut self, parts: &[Op; 3], frame: &mut Frame) -> Result<Type> {
        let [test, then, otherwise] = parts;
        let t = self.expr(test, frame)?;
        boolean(&t)?;
        Ok(Type::choice(vec![self.expr(then, frame)?, self.expr(otherwise, frame)?]))
    }

    /// [XQueryFS §4.12.3] `cast as`: of the target type, optional if `?`.
    fn cast(&mut self, operand: &Op, target: &super::super::plan::CastTarget, frame: &mut Frame) -> Result<Type> {
        self.cast_operand(operand, target.optional, frame)?;
        // [XQuery §3.12.3] of a user-defined type, a value of it.
        let to = target.defined.as_ref().map_or(Atom::Of(target.to), |d| Atom::Defined(Arc::clone(d)));
        Ok(Type::occur(Type::Item(Item::Atomic(to)), if target.optional { Occ::Optional } else { Occ::One }))
    }

    /// Each of `ops` typed.
    pub(super) fn all(&mut self, ops: &[Op], frame: &mut Frame) -> Result<Vec<Type>> {
        ops.iter().map(|o| self.expr(o, frame)).collect()
    }

    /// An arithmetic operand: `fs:convert-operand(fn:data(op), 1.0E0)`
    /// [XQueryFS §4.4], its untyped values doubles.
    fn operand(&mut self, op: &Op, frame: &mut Frame) -> Result<Type> {
        let t = data(&self.expr(op, frame)?)?;
        super::values::convert_untyped(&t, &Type::atomic(AtomicType::Double))
    }

    /// [XQueryFS §4.4, Appendix B.2] A chain of arithmetic operators, each
    /// applied to what those before it make.
    fn arithmetic(&mut self, first: &Op, rest: &[(Arith, Op)], frame: &mut Frame) -> Result<Type> {
        let mut t = self.operand(first, frame)?;
        for (op, e) in rest {
            let right = self.operand(e, frame)?;
            let what = arith_name(*op);
            t = binary(&t, &right, what, &|a, b| arithmetic(*op, a, b))?;
        }
        Ok(t)
    }

    /// [XQueryFS §4.5] A comparison: of values, `fs:op` of its operands
    /// atomized and converted [XQueryFS §4.5.1]; general, of each pair of
    /// the atomized operands' item types [XQueryFS §4.5.2]; of nodes,
    /// [XQueryFS Appendix B.2] of two optional nodes.
    fn compare(&mut self, c: &CompareOp, frame: &mut Frame) -> Result<Type> {
        let left = self.expr(&c.left, frame)?;
        let right = self.expr(&c.right, frame)?;
        let equality = is_equality(c.op);
        match c.op {
            Comparator::Value(_) => {
                let string = Atom::Of(AtomicType::String);
                let to_string = |t: &Type| super::values::convert_untyped(&data(t)?, &Type::atomic(AtomicType::String));
                let values = move |a: &Atom, b: &Atom| {
                    comparable(&convert(a, &string), &convert(b, &string), equality).then_some(AtomicType::Boolean)
                };
                binary(&to_string(&left)?, &to_string(&right)?, "a value comparison", &values)
            }
            Comparator::General(_) => {
                let pairs = move |a: &Atom, b: &Atom| comparable(&convert(a, b), &convert(b, a), equality).then_some(AtomicType::Boolean);
                let (l, r) = (data(&left)?, data(&right)?);
                for a in prime(&l)? {
                    for b in prime(&r)? {
                        binary(&Type::Item(a.clone()), &Type::Item(b.clone()), "a general comparison", &pairs)?;
                    }
                }
                Ok(Type::boolean())
            }
            Comparator::Node(_) => {
                let node = Type::occur(Type::of_prime(node_items(), Occ::One), Occ::Optional);
                for t in [&left, &right] {
                    if !subtype(t, &node)? {
                        return Err(type_error(&format!(
                            "an operand of a node comparison is of {t}, not of node()? [XQueryFS Appendix B.2]"
                        )));
                    }
                }
                let optional = quantifier(&left)?.zero() || quantifier(&right)?.zero();
                Ok(Type::occur(Type::boolean(), if optional { Occ::Optional } else { Occ::One }))
            }
        }
    }

    /// [XQueryFS §7.2.14] `union`: its operands' items, nodes alone, in
    /// one choice of their quantifiers combined.
    fn union(&mut self, ops: &[Op], frame: &mut Frame) -> Result<Type> {
        let types = self.all(ops, frame)?;
        for t in &types {
            nodes_only(t, "union")?;
        }
        factored(&Type::Seq(types))
    }

    /// [XQueryFS §7.2.14] `intersect` and `except`: of the operands' items,
    /// of `except` the first's, optional.
    fn intersect_except(&mut self, first: &Op, rest: &[(super::super::ast::SetOp, Op)], frame: &mut Frame) -> Result<Type> {
        let mut t = self.expr(first, frame)?;
        nodes_only(&t, "intersect or except")?;
        for (set, e) in rest {
            let other = self.expr(e, frame)?;
            nodes_only(&other, "intersect or except")?;
            let both = if *set == super::super::ast::SetOp::Intersect { Type::seq(vec![t, other]) } else { t };
            t = Type::occur(factored(&both)?, Occ::Optional);
        }
        Ok(t)
    }

    /// [XQueryFS §4.12.3, §4.12.4] A cast's operand: `let $v as
    /// xdt:anyAtomicType(?) := fn:data(op)`, one atomic value, or none if
    /// `optional`.
    fn cast_operand(&mut self, op: &Op, optional: bool, frame: &mut Frame) -> Result<()> {
        let t = data(&self.expr(op, frame)?)?;
        let atomic = Type::occur(Type::Item(Item::Atomic(Atom::Any)), if optional { Occ::Optional } else { Occ::One });
        if subtype(&t, &atomic)? {
            return Ok(());
        }
        Err(type_error(&format!("the operand of a cast is of {t}, not of {atomic} [XQueryFS §4.12.3]")))
    }

    /// [XQueryFS §4.13] `validate`: of an element or a document of one
    /// element, of what the in-scope schema definitions declare of it;
    /// a document validated a document of it.
    fn validate(&mut self, operand: &Op, frame: &mut Frame) -> Result<Type> {
        let t = self.expr(operand, frame)?;
        let element = super::sequence::any_element();
        let document = Item::Document(Box::new(Type::All(vec![
            Type::Item(element.clone()),
            Type::of_prime(vec![Item::Pi, Item::Comment], Occ::Star),
        ])));
        if !subtype(&t, &Type::of_prime(vec![element, document], Occ::One))? {
            return Err(super::super::error::error(
                super::super::error::ErrorCode::XQTY0030,
                &format!("the operand of validate is of {t}, not one element or document node of one element [XQueryFS §4.13]"),
            ));
        }
        let mut out = Vec::new();
        for item in prime(&t)? {
            out.push(self.validated(&item)?);
        }
        Ok(Type::choice(out))
    }

    /// [XQueryFS §8.6.1] What validating an element or document of item
    /// type `item` makes: of each global element declaration of the
    /// in-scope schema definitions of its name, the element it declares;
    /// of none, an element of `xs:anyType`, as lax validation makes it.
    fn validated(&self, item: &Item) -> Result<Type> {
        let (name, document) = match item {
            Item::Element(n) => (n.name.clone(), false),
            Item::Document(content) => {
                let names: Vec<_> = prime(content)?
                    .into_iter()
                    .filter_map(|i| if let Item::Element(n) = i { Some(n.name.clone()) } else { None })
                    .collect();
                (names.into_iter().next().unwrap_or(super::types::Name::ANY), true)
            }
            Item::Attribute(_) | Item::Text | Item::Comment | Item::Pi | Item::Atomic(_) => return Ok(Type::None),
        };
        let mut found: Vec<Type> = self.schemas.iter().map(|s| super::expand::declared_elements(&s.schema, Some(&name))).collect();
        found.push(Type::Item(super::sequence::any_element()));
        let element = Type::choice(found);
        if !document {
            return Ok(element);
        }
        Ok(Type::Item(Item::Document(Box::new(Type::All(vec![element, Type::of_prime(vec![Item::Pi, Item::Comment], Occ::Star)])))))
    }

    /// [XQueryFS §4.2] A path: its first step of the focus, or of the root
    /// of the focus's tree, `fn:root(self::node()) treat as
    /// document-node()`, a focus of documents itself; each step after of each node the steps before it
    /// give, as its focus [XQueryFS §4.2, normalization of `/`].
    fn path(&mut self, p: &PathOp, frame: &mut Frame) -> Result<Type> {
        let mut current = match p.start {
            PathStart::Context => None,
            PathStart::Root => {
                let focus = frame.focus.clone().ok_or_else(|| super::no_focus("/"))?;
                step(&focus, super::super::plan::Axis::SelfNode, &super::super::plan::StepTest::Kind(super::super::types::NodeKind::Any))?;
                // [XQuery §5.2.3.1] [XQueryFS §6.1.1] a static typing
                // extension: a document is its own tree's root, so the root
                // of a focus of documents alone is of its type, more precise
                // than fn:root's `document` [XQueryFS §7.2].
                let documents = prime(&focus)?.iter().all(|i| matches!(i, Item::Document(_)));
                Some(if documents { focus } else { Type::Item(any_document()) })
            }
        };
        for s in &p.steps {
            current = Some(match current {
                None => self.step(s, frame)?,
                Some(before) => self.after(&before, s, frame)?,
            });
        }
        current.map_or_else(|| Ok(Type::Item(any_document())), Ok)
    }

    /// `before / s` [XQueryFS §4.2]: `before` nodes alone \[err:XPTY0019\],
    /// `s` of each as the focus, its type of `before`'s quantifier, its
    /// nodes in document order, nodes or atomic values alone
    /// \[err:XPTY0018\].
    fn after(&mut self, before: &Type, s: &StepOp, frame: &mut Frame) -> Result<Type> {
        let nodes = Type::occur(Type::of_prime(node_items(), Occ::One), Occ::Star);
        if !subtype(before, &nodes)? {
            return Err(super::super::error::error(
                super::super::error::ErrorCode::XPTY0019,
                &format!("a step of a path is of {before}, not of node()* [XQueryFS §4.2] [XQuery §3.2]"),
            ));
        }
        let saved = frame.focus.replace(Type::of_prime(prime(before)?, Occ::One));
        let typed = self.step(s, frame);
        frame.focus = saved;
        let t = Type::occur(typed?, quantifier(before)?);
        if subtype(&t, &nodes)? {
            return factored(&t);
        }
        if subtype(&t, &Type::occur(Type::Item(Item::Atomic(Atom::Any)), Occ::Star))? {
            return Ok(t);
        }
        Err(super::super::error::error(
            super::super::error::ErrorCode::XPTY0018,
            &format!("the last step of a path is of {t}, of nodes and atomic values [XQueryFS §7.1.4] [XQuery §3.2]"),
        ))
    }

    /// A step [XQueryFS §4.2.1]: an axis step of the focus, its nodes in
    /// document order, or a filter expression; then its predicates.
    fn step(&mut self, s: &StepOp, frame: &mut Frame) -> Result<Type> {
        match s {
            StepOp::Axis(axis, test, predicates) => {
                let focus = frame.focus.clone().ok_or_else(|| super::no_focus("an axis step"))?;
                let selected = step(&focus, *axis, test)?;
                if is_empty(&selected)? {
                    let shown = super::axes::step_text(*axis, test);
                    let why = format!("the step {shown} of a context item of {focus} selects nothing [XQuery §2.3.1] [XQueryFS §4]");
                    return Err(super::empty_type(&why));
                }
                let t = factored(&selected)?;
                self.predicates(t, predicates, frame)
            }
            StepOp::Filter(primary, predicates) => {
                let t = self.expr(primary, frame)?;
                self.predicates(t, predicates, frame)
            }
        }
    }

    /// [XQueryFS §4.2.2, §4.3.2] `t` filtered by `predicates`, each in
    /// turn: a numeric literal, or fn:last(), picks an item
    /// (`functions::pick`); any other is typed with each item of `t` as
    /// the focus, its type one whose effective boolean value is defined or
    /// a number, and leaves `t`'s items, optional.
    fn predicates(&mut self, t: Type, predicates: &[Op], frame: &mut Frame) -> Result<Type> {
        let mut t = t;
        for p in predicates {
            if is_numeric_literal(p) || calls(p, LAST) {
                t = pick(&t, if is_one(p) { Pick::First } else { Pick::One })?;
                continue;
            }
            let saved = frame.focus.replace(Type::of_prime(prime(&t)?, Occ::One));
            let typed = self.expr(p, frame);
            frame.focus = saved;
            let pt = typed?;
            if !subtype(&pt, &Type::Item(Item::Atomic(Atom::Numeric)))? {
                boolean(&pt)?;
            }
            t = Type::of_prime(prime(&t)?, quantifier(&t)?.times(Occ::Optional));
        }
        Ok(t)
    }

    /// [XQueryFS §4.8] A FLWOR expression: each clause binding its variable
    /// as `for` or `let` does, the `where` an `if` of its result, the
    /// `order by` keys each compared by `gt` with itself [XQueryFS §4.8.4];
    /// its type its result's, times the quantifier of each `for`'s input,
    /// optional of a `where`.
    fn flwor(&mut self, f: &FlworOp, frame: &mut Frame) -> Result<Type> {
        let mut occ = Occ::One;
        for clause in &f.clauses {
            match clause {
                Clause::For(slot, position, declared, input) => {
                    let t = self.expr(input, frame)?;
                    let each = Self::bound_each(&t, declared.as_ref())?;
                    set(frame, *slot, each);
                    if let Some(p) = position {
                        set(frame, *p, Type::atomic(AtomicType::Integer));
                    }
                    occ = occ.times(quantifier(&t)?);
                }
                Clause::Let(slot, declared, value) => {
                    let t = self.expr(value, frame)?;
                    let bound = Self::bound_whole(t, declared.as_ref())?;
                    set(frame, *slot, bound);
                }
            }
        }
        if let Some(condition) = &f.condition {
            let t = self.expr(condition, frame)?;
            boolean(&t)?;
            occ = occ.times(Occ::Optional);
        }
        for key in &f.order {
            let t = data(&self.expr(&key.key, frame)?)?;
            let one = Type::of_prime(prime(&t)?, Occ::One);
            let string = Atom::Of(AtomicType::String);
            let gt = |a: &Atom, b: &Atom| comparable(&convert(a, &string), &convert(b, &string), false).then_some(AtomicType::Boolean);
            binary(&one, &one, "an order spec's gt", &gt)?;
        }
        Ok(Type::occur(self.expr(&f.result, frame)?, occ))
    }

    /// [XQueryFS §4.8.2, §4.11] The type of a `for` or quantified variable
    /// over `t`: its declared type, of which each item of `t` is, else
    /// `prime(t)`.
    fn bound_each(t: &Type, declared: Option<&super::super::types::SeqType>) -> Result<Type> {
        let each = Type::of_prime(prime(t)?, Occ::One);
        let Some(d) = declared else { return Ok(each) };
        let d = normalized(d);
        if !subtype(&each, &d)? {
            return Err(type_error(&format!("a variable declared {d} is bound to items of {each} [XQueryFS §4.8.2]")));
        }
        Ok(d)
    }

    /// [XQueryFS §4.8.3] The type of a `let` variable bound to `t`: its
    /// declared type, of which `t` is a subtype, else `t`.
    fn bound_whole(t: Type, declared: Option<&super::super::types::SeqType>) -> Result<Type> {
        let Some(d) = declared else { return Ok(t) };
        let d = normalized(d);
        if !subtype(&t, &d)? {
            return Err(type_error(&format!("a variable declared {d} is bound to a value of {t} [XQueryFS §4.8.3]")));
        }
        Ok(d)
    }

    /// [XQueryFS §4.11] `some` and `every`: each variable bound as `for`
    /// binds it, the test's effective boolean value defined.
    fn quantified(&mut self, q: &QuantifiedOp, frame: &mut Frame) -> Result<Type> {
        for (slot, declared, input) in &q.bindings {
            let t = self.expr(input, frame)?;
            let each = Self::bound_each(&t, declared.as_ref())?;
            set(frame, *slot, each);
        }
        let t = self.expr(&q.test, frame)?;
        boolean(&t)?;
        Ok(Type::boolean())
    }

    /// [XQueryFS §4.12.2] `typeswitch`: each case's variable of its
    /// sequence type, the default's of the operand's type; of the union of
    /// the clauses' types.
    fn typeswitch(&mut self, t: &TypeswitchOp, frame: &mut Frame) -> Result<Type> {
        let operand = self.expr(&t.operand, frame)?;
        let mut types = Vec::with_capacity(t.cases.len().saturating_add(1));
        for (slot, matched, result) in &t.cases {
            if let Some(s) = slot {
                set(frame, *s, normalized(matched));
            }
            types.push(self.expr(result, frame)?);
        }
        if let Some(s) = t.default.0 {
            set(frame, s, operand);
        }
        types.push(self.expr(&t.default.1, frame)?);
        Ok(Type::choice(types))
    }
}

/// `slot` of `frame` given type `t`.
fn set(frame: &mut Frame, slot: usize, t: Type) {
    if let Some(s) = frame.slots.get_mut(slot) {
        *s = Some(t);
    }
}

/// [XQueryFS §7.2.14] The error unless `t` is of nodes alone, of the
/// operand of `operator`.
fn nodes_only(t: &Type, operator: &str) -> Result<()> {
    let nodes = Type::occur(Type::of_prime(node_items(), Occ::One), Occ::Star);
    if subtype(t, &nodes)? {
        return Ok(());
    }
    Err(type_error(&format!("an operand of {operator} is of {t}, not of node()* [XQueryFS §7.2.14] [XQuery §3.3.3]")))
}

/// [XQueryFS Appendix B.2] The type of a unary operator of a number `a`:
/// `a`'s numeric type.
fn unary(a: &Atom, _: &Atom) -> Option<AtomicType> {
    super::operators::rank(a).map(super::operators::numeric_at)
}

/// The operator `op` as XQuery writes it, for messages.
fn arith_name(op: Arith) -> &'static str {
    match op {
        Arith::Add => "+",
        Arith::Subtract => "-",
        Arith::Multiply => "*",
        Arith::Divide => "div",
        Arith::IntegerDivide => "idiv",
        Arith::Modulo => "mod",
    }
}

impl Statics<'_> {
    /// [XQueryFS §4] Whether `op` may be of the empty type: `()`, fn:data
    /// of `()`, or a call of a function whose type is the empty type.
    fn exempt(&self, op: &Op) -> bool {
        let Op::Call(call) = op else { return matches!(op, Op::Empty) };
        let declared_empty = match &call.callee {
            Callee::Declared(at) => self.functions.get(*at).is_some_and(|(_, result)| *result == Type::Empty),
            Callee::Library(f) => *f.result() == super::super::types::SeqType::Empty,
        };
        declared_empty || (calls(op, DATA) && call.arguments.iter().all(|a| matches!(a, Op::Empty)))
    }
}

/// [XQuery §2.3.1] The error of `op` of the empty type \[err:XPST0005\].
fn empty(op: &Op) -> crate::error::SqlError {
    super::empty_type(&format!("{} is of the empty type [XQuery §2.3.1] [XQueryFS §4]", describe(op)))
}

/// What kind of expression `op` is, for a message.
fn describe(op: &Op) -> &'static str {
    match op {
        Op::Empty => "()",
        Op::Atomic(_) => "a literal",
        Op::Sequence(_) => "a sequence",
        Op::Local(_) | Op::Global(_) => "a variable reference",
        Op::ContextItem => "the context item",
        Op::Path(_) => "a path",
        Op::Filter(..) => "a filter expression",
        Op::Range(_) => "a range",
        Op::Arithmetic(..) | Op::Unary(..) => "an arithmetic expression",
        Op::Union(_) | Op::IntersectExcept(..) => "a union, intersect or except",
        Op::Compare(_) => "a comparison",
        Op::And(_) | Op::Or(_) => "a logical expression",
        Op::If(_) => "a conditional expression",
        Op::Flwor(_) => "a FLWOR expression",
        Op::Quantified(_) => "a quantified expression",
        Op::Typeswitch(_) => "a typeswitch expression",
        Op::InstanceOf(..) | Op::Treat(..) | Op::Castable(..) | Op::Cast(..) => "an expression on a sequence type",
        Op::Call(_) => "a function call",
        Op::Validate(..) => "a validate expression",
        Op::Element(_)
        | Op::ComputedElement(_)
        | Op::Attribute(..)
        | Op::Document(_)
        | Op::Text(_)
        | Op::Comment(_)
        | Op::DirectComment(_)
        | Op::Pi(_) => "a constructor",
    }
}
