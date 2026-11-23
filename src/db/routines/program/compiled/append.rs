//! \[DESIGN\] RULES.md D914: the assignment `SET A = A || ARRAY[x]` of a
//! compiled program, run in place. `A` is an SQL variable or parameter of
//! an array type, assigned as a whole, and the source is the
//! concatenation [ISO/IEC 9075-2:2023 §6.47] of `A` itself and an array
//! of one element [§6.49].
//!
//! As bound, it reads `A`, makes an array of its elements and `x`, and
//! assigns that to `A` element by element [§9.2 GR3b xxii]. While `A`
//! holds an array of its type with room for one element more, that
//! leaves `A`'s elements as they are and `x`, cast and assigned to the
//! element type, after them. So `x` alone is evaluated, cast and
//! assigned, and it is appended to the array the frame holds. An array
//! that is null, full or not of its type is assigned as bound, which
//! raises what its rules raise.

use std::sync::Arc;

use super::code::Assign;
use super::run::Exec;
use super::valued::{RowOf, Store, Stores, Valued, stored};
use crate::db::Completion;
use crate::error::{Code, Result, SqlError};
use crate::exec::{Framed, eval};
use crate::functions::ArrayOperation;
use crate::logical::Expr;
use crate::storage::Row;
use crate::value::types::array::{ArrayType, appended};
use crate::value::{DataType, Value, cast, held};

/// An assignment that appends one element to its target.
#[derive(Debug)]
pub(super) struct Append {
    /// The frame's slot of the array, and its type.
    slot: usize,
    array: Arc<ArrayType>,
    /// The element appended, over the outer values of `outer`.
    element: Expr,
    outer: Vec<Expr>,
    /// The outer value that is the array, if the element reads it not:
    /// it is then not read out of the frame.
    own: Option<usize>,
    /// The element type the constructor's array is cast to an array of,
    /// if it is cast.
    cast: Option<DataType>,
    /// Where the target and the source are written.
    written: usize,
    at: usize,
}

impl Append {
    /// What the assignment of `source` to the targets stored as `stores`
    /// appends, if it is of the shape above.
    pub(super) fn of(source: Option<&Valued>, stores: &Stores) -> Option<Append> {
        let (Some(Valued::Row(r)), [Store::Slot { at, data_type, written }]) = (source, stores.0.as_slice()) else { return None };
        let DataType::Array(array) = data_type else { return None };
        let [Expr::Call(f, args)] = r.row.as_slice() else { return None };
        let ([left, right], Some(ArrayOperation::Concatenation)) = (args.as_slice(), f.0.array_operation()) else { return None };
        let own = own_array(left, array).filter(|i| names_slot(r, *i, (*at, data_type)))?;
        let (element, cast) = constructed(right, array)?;
        let reads_own = reads_outer(&element, own);
        Some(Append {
            slot: *at,
            array: Arc::clone(array),
            element,
            outer: r.outer.clone(),
            own: (!reads_own).then_some(own),
            cast,
            written: *written,
            at: r.at,
        })
    }
}

/// The outer reference `left` is, as it is or cast to an array type of
/// `array`'s element type and of no smaller maximum cardinality, which
/// leaves an array of `array`'s type as it is.
fn own_array(left: &Expr, array: &ArrayType) -> Option<usize> {
    let inner = if let Expr::Cast(x, to) = left {
        let DataType::Array(wide) = to else { return None };
        if wide.element != array.element || wide.max < array.max {
            return None;
        }
        &**x
    } else {
        left
    };
    if let Expr::Outer(i, _) = inner { Some(*i) } else { None }
}

/// Whether the outer expression `i` of `r` is the slot `at` of the frame,
/// of the type `data_type`.
fn names_slot(r: &RowOf, i: usize, (at, data_type): (usize, &DataType)) -> bool {
    if let Some(Expr::Dynamic(slot)) = r.outer.get(i) { slot.index == at && slot.data_type == *data_type } else { false }
}

/// Of `right`, an array constructor of one element, as it is if its
/// element type is `array`'s, or cast to an array of that element type:
/// the element, and the element type it is cast to, if it is cast.
fn constructed(right: &Expr, array: &ArrayType) -> Option<(Expr, Option<DataType>)> {
    let (inner, cast) = if let Expr::Cast(x, to) = right {
        let DataType::Array(wide) = to else { return None };
        if wide.element != array.element {
            return None;
        }
        (&**x, Some(wide.element.clone()))
    } else {
        (right, None)
    };
    let Expr::Call(g, items) = inner else { return None };
    let ([element], Some(ArrayOperation::Constructor)) = (items.as_slice(), g.0.array_operation()) else { return None };
    let Ok(DataType::Array(made)) = g.0.return_type(&[]) else { return None };
    if cast.is_none() && made.element != array.element {
        return None;
    }
    Some((element.clone(), cast))
}

/// Whether `e` reads the outer value `own`, at any depth.
fn reads_outer(e: &Expr, own: usize) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        if let Expr::Outer(i, _) = next
            && *i == own
        {
            return true;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    false
}

impl Exec<'_> {
    /// The assignment `a`, which appends `ap`, run in place, if its
    /// target holds an array of its type with room for one element more;
    /// none if it is to be assigned as bound.
    ///
    /// # Errors
    ///
    /// The element's, its cast's and its store assignment's, as the
    /// assignment as bound raises them.
    pub(super) fn appended(&mut self, a: &Assign, ap: &Append) -> Result<Option<Completion>> {
        let Some(Value::Array(elements)) = self.frame.dynamic_values().get(ap.slot) else { return Ok(None) };
        let room = usize::try_from(ap.array.max).is_ok_and(|max| elements.len() < max);
        if !room || !elements.iter().all(|e| held(e, &ap.array.element)) {
            return Ok(None);
        }
        let value = self.element(ap).map_err(|e| e.or_at(ap.at))?;
        self.frame.set_routine(Arc::clone(&a.scope));
        let target = a.targets.first().map_or(a.at, |t| t.spec.at);
        let value = stored(value, (&ap.array.element, ap.written)).map_err(|e| e.or_at(target).or_at(a.at))?;
        let Value::Array(own) = self.frame.take_slot(ap.slot)? else {
            return Err(SqlError::new(Code::Api, "[DESIGN] the array an assignment appends to is no array"));
        };
        self.frame.set_slot(ap.slot, appended(own, value))?;
        Ok(Some(Completion::Success))
    }

    /// The element `ap` appends: evaluated over its outer values, the
    /// array's own left unread if the element reads it not, and cast to
    /// the element type as the constructor's array is.
    fn element(&self, ap: &Append) -> Result<Value> {
        let none = Row::new();
        let over_frame = Framed { inner: &self.hosted, frame: self.frame.dynamic_values(), outer: &[] };
        let mut outer = Vec::with_capacity(ap.outer.len());
        for (i, e) in ap.outer.iter().enumerate() {
            outer.push(if ap.own == Some(i) { Value::Null } else { eval(e, &none, &over_frame)? });
        }
        let framed = Framed { inner: &self.hosted, frame: self.frame.dynamic_values(), outer: &outer };
        let value = eval(&ap.element, &none, &framed)?;
        match &ap.cast {
            Some(to) => cast(value, to),
            None => Ok(value),
        }
    }
}
