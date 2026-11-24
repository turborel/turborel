//! \[DESIGN\] A run of a program as a frame of its debugger's stack
//! (RULES.md D1003, D1007, D1008): pushed as its first statement is
//! looked at with a debugger attached, popped as the run ends.
//!
//! Each statement is found by where it is held, as a program kept finds
//! its statements' plans (`routines/kept.rs`): the steps are walked once,
//! as the frame is pushed, in the order the binder noted where each
//! begins (`Program::places`), and each step's address is given its place.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Frame, Kind, Link, Origin, Reason};
use crate::catalog::{Catalog, owner_of, sole};
use crate::logical::{Program, Step};

/// A run of a program, a frame of the stack of `link`.
#[derive(Debug)]
pub(crate) struct Tracked {
    link: Arc<Link>,
    /// Its frame's index in the stack.
    index: usize,
    /// Whether the debugger may see into it (RULES.md D1008).
    visible: bool,
    /// What a breakpoint names its routine or trigger by.
    name: Option<crate::catalog::QualifiedName>,
    /// Where each step begins, by the step's address.
    places: HashMap<usize, usize>,
}

/// Where `step` is held: the same while its program is.
fn address(step: &Step) -> usize {
    std::ptr::from_ref(step).addr()
}

/// \[EXTENSION\] Whether `debugger` may see into a run of what `origin`
/// names in `catalog`: as the owner of its schema, or of a routine as one
/// who holds DEBUG on it (RULES.md D1008). A control statement run directly
/// is of no routine, and is seen into by none.
pub(crate) fn may_debug(catalog: &dyn Catalog, debugger: &str, origin: Option<&Origin>) -> bool {
    let Some(origin) = origin else { return false };
    let who = sole(catalog, debugger);
    match origin.kind {
        Kind::Trigger => who.includes(owner_of(catalog, &origin.name.auth)),
        Kind::Procedure | Kind::Function => {
            catalog.routine(&origin.name).is_some_and(|r| r.debug_of(who, owner_of(catalog, &r.name.auth)).0)
        }
    }
}

impl Tracked {
    /// A run of `program`, pushed on the stack of `link`, seen into if
    /// `visible`; none if the stack is another thread's or the debugger has
    /// gone.
    pub(crate) fn enter(link: Arc<Link>, program: &Program, visible: bool) -> Option<Tracked> {
        let origin = program.origin.clone();
        let name = origin.as_ref().map(|o| o.name.clone());
        let index = link.push(Frame { origin, visible, ..Frame::default() })?;
        let mut places = HashMap::new();
        let mut work = vec![&program.body];
        let mut at = program.places.iter();
        while let Some(step) = work.pop() {
            places.insert(address(step), at.next().copied().unwrap_or_default());
            // The steps it holds, the first on top.
            work.extend(step.steps().into_iter().rev());
        }
        Some(Tracked { link, index, visible, name, places })
    }

    /// \[EXTENSION\] A statement of a triggered action of `origin` run
    /// alone, not as a program's step, pushed on the stack of `link` as a
    /// frame of its own, seen into if `visible`; none as `enter` (RULES.md
    /// D1014).
    pub(crate) fn alone(link: Arc<Link>, origin: Origin, visible: bool) -> Option<Tracked> {
        let name = Some(origin.name.clone());
        let index = link.push(Frame { origin: Some(origin), visible, ..Frame::default() })?;
        Some(Tracked { link, index, visible, name, places: HashMap::new() })
    }

    /// Whether the debugger may see into it.
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    /// Where `step` begins, in characters from the beginning of its text.
    pub(crate) fn place(&self, step: &Step) -> usize {
        self.places.get(&address(step)).copied().unwrap_or_default()
    }

    /// Its frame as `f` leaves it.
    pub(crate) fn update(&self, f: impl FnOnce(&mut Frame)) {
        self.link.update(self.index, f);
    }

    /// Why its statement beginning at `at` pauses, if it does.
    pub(crate) fn stops(&self, at: usize) -> Option<Reason> {
        self.link.stops(self.index, self.name.as_ref(), at)
    }

    /// Its frame's index, and its link.
    pub(crate) fn link(&self) -> (usize, &Link) {
        (self.index, &self.link)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.link.pop(self.index);
    }
}
