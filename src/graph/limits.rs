//! \[DESIGN\] A path search's resource limits [GS §20]: the longest path,
//! the most paths of a partition and the most states, each failing the
//! search with GRF-0061 or, ON LIMIT TRUNCATE, ending it with a warning
//! [GS §15 notes]; the memory it holds, a work area's and past it the
//! memory budget's (RULES.md D38); and its cancellation, looked at as
//! each state is expanded (RULES.md D44).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Code, Result, SqlError, SqlState};
use crate::memory::{MemoryBudget, Reservation};
use crate::sync::lock_builtin;
use crate::txn::Stop;

/// \[GS §22\] GRF-0061's SQLSTATE: class 54, an implementation-defined
/// class [ISO/IEC 9075-2:2023 §24.1], subclass its number.
pub const LIMIT_STATE: &str = "54061";

/// \[GS §20\] The SQLSTATE of the warning a search ended ON LIMIT
/// TRUNCATE raises: a warning of an implementation-defined subclass
/// [ISO/IEC 9075-2:2023 §24.1], subclass GRF-0061's number.
pub const TRUNCATED_STATE: &str = "01W61";

/// \[GS §22\] GRF-0034's SQLSTATE: a data exception of an
/// implementation-defined subclass [ISO/IEC 9075-2:2023 §24.1].
pub const COST_STATE: &str = "22W34";

/// \[DESIGN\] The bytes a search reserves of the memory budget at a time,
/// past its work area, so it does not reserve at every state.
const RESERVE_STEP: usize = 1 << 20;

/// \[GS §20\] What a limit reached does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OnLimit {
    /// Fails the search with GRF-0061.
    #[default]
    Error,
    /// Ends the search with what it found, and a warning.
    Truncate,
}

/// \[GS §20\] A path search's limits, none if not set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// MAX_PATH_LENGTH: the most edges of a path.
    pub max_path_length: Option<usize>,
    /// MAX_PATHS_PER_PARTITION: the most paths of one (start, end) pair.
    pub max_paths_per_partition: Option<usize>,
    /// MAX_SEARCH_STATES: the most states the search makes.
    pub max_search_states: Option<usize>,
    /// What a limit reached does.
    pub on_limit: OnLimit,
}

/// \[DESIGN\] The memory a search may hold: `work` bytes, and past them
/// what it reserves of `budget`, refused (SQLCODE -918) past its limit or
/// without one.
#[derive(Clone, Debug)]
pub struct Area {
    /// The bytes it holds without reserving, MEMORY.WORK_BYTES.
    pub work: usize,
    /// The budget it reserves of past them.
    pub budget: Option<Arc<MemoryBudget>>,
}

impl Default for Area {
    fn default() -> Area {
        Area { work: usize::MAX, budget: None }
    }
}

/// The error of a limit reached ON LIMIT ERROR.
fn limit_error(name: &str, limit: usize) -> SqlError {
    let state = SqlState::parse(LIMIT_STATE).unwrap_or_else(|| SqlState::of(Code::Unsupported));
    SqlError::new(Code::Unsupported, format!("[GS §20] GRF-0061: the path search passed {name}, {limit}")).with_state(state)
}

/// \[GS §7.2\] GRF-0034: a cost not a non-negative number.
pub(super) fn cost_error(cost: f64) -> SqlError {
    let state = SqlState::parse(COST_STATE).unwrap_or_else(|| SqlState::of(Code::Data));
    SqlError::new(Code::Data, format!("[GS §7.2] GRF-0034: the cost of an edge, {cost}, is not a non-negative number")).with_state(state)
}

/// What the parts of one search have used together: they may run on
/// several threads.
#[derive(Default)]
struct Used {
    states: AtomicUsize,
    bytes: AtomicUsize,
    reserved: Mutex<Option<Reservation>>,
    truncated: AtomicBool,
}

/// What a search has used of its limits, its memory and its time. Its
/// parts, run apart, share one count.
pub(super) struct Meter {
    limits: Limits,
    stop: Option<Arc<Stop>>,
    area: Area,
    used: Arc<Used>,
}

impl Meter {
    /// A meter of `limits` and `area`, looking at `stop`.
    pub(super) fn new(limits: Limits, stop: Option<Arc<Stop>>, area: Area) -> Meter {
        Meter { limits, stop, area, used: Arc::default() }
    }

    /// A meter for a part of the search, run apart, counting with it.
    pub(super) fn part(&self) -> Meter {
        Meter { limits: self.limits, stop: self.stop.clone(), area: self.area.clone(), used: Arc::clone(&self.used) }
    }

    /// What cancels the statement, for parallel workers to look at.
    pub(super) fn stop(&self) -> Option<&Arc<Stop>> {
        self.stop.as_ref()
    }

    /// Whether a limit ended the search ON LIMIT TRUNCATE.
    pub(super) fn truncated(&self) -> bool {
        self.used.truncated.load(Ordering::Acquire)
    }

    /// Whether the statement was cancelled (RULES.md D44).
    ///
    /// # Errors
    ///
    /// SQLCODE -920 if it was.
    pub(super) fn check(&self) -> Result<()> {
        self.stop.as_ref().map_or(Ok(()), |stop| stop.check())
    }

    /// A limit `name` of `limit` is reached: an error, or the search
    /// truncated, false.
    fn reached(&self, name: &str, limit: usize) -> Result<bool> {
        match self.limits.on_limit {
            OnLimit::Error => Err(limit_error(name, limit)),
            OnLimit::Truncate => {
                self.used.truncated.store(true, Ordering::Release);
                Ok(false)
            }
        }
    }

    /// Counts a new state of `bytes`: whether the search goes on.
    ///
    /// # Errors
    ///
    /// GRF-0061 past MAX_SEARCH_STATES ON LIMIT ERROR; -918 if the memory
    /// past the work area is refused; -920 if cancelled.
    pub(super) fn state(&mut self, bytes: usize) -> Result<bool> {
        self.check()?;
        if self.truncated() {
            return Ok(false);
        }
        let states = self.used.states.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        if let Some(limit) = self.limits.max_search_states.filter(|&limit| states > limit) {
            return self.reached("MAX_SEARCH_STATES", limit);
        }
        let held = self.used.bytes.fetch_add(bytes, Ordering::AcqRel).saturating_add(bytes);
        let past = held.saturating_sub(self.area.work);
        if past > 0 {
            self.reserve(past)?;
        }
        Ok(true)
    }

    /// Reserves at least `past` bytes of the budget.
    fn reserve(&self, past: usize) -> Result<()> {
        let mut reserved = lock_builtin(&self.used.reserved);
        if reserved.as_ref().is_some_and(|r| r.bytes() >= past) {
            return Ok(());
        }
        let want = past.div_ceil(RESERVE_STEP).saturating_mul(RESERVE_STEP);
        match (reserved.as_mut(), &self.area.budget) {
            (Some(held), _) => held.resize(want),
            (None, Some(budget)) => {
                *reserved = Some(budget.reserve(want, "a path search")?);
                Ok(())
            }
            (None, None) => Err(SqlError::new(
                Code::Resources,
                format!("[DESIGN] a path search needs more than its work area's {} bytes", self.area.work),
            )),
        }
    }

    /// Whether a path of `length` edges may be searched.
    ///
    /// # Errors
    ///
    /// GRF-0061 past MAX_PATH_LENGTH ON LIMIT ERROR.
    pub(super) fn length(&mut self, length: usize) -> Result<bool> {
        match self.limits.max_path_length.filter(|&limit| length > limit) {
            Some(limit) => self.reached("MAX_PATH_LENGTH", limit),
            None => Ok(true),
        }
    }

    /// Whether a partition may hold `paths` paths.
    ///
    /// # Errors
    ///
    /// GRF-0061 past MAX_PATHS_PER_PARTITION ON LIMIT ERROR.
    pub(super) fn paths(&mut self, paths: usize) -> Result<bool> {
        match self.limits.max_paths_per_partition.filter(|&limit| paths > limit) {
            Some(limit) => self.reached("MAX_PATHS_PER_PARTITION", limit),
            None => Ok(true),
        }
    }

    /// Raises the truncation warning, if the search was truncated, on the
    /// statement's warnings [ISO/IEC 9075:1992 §3.3.4.1].
    pub(super) fn warn(&self) {
        if let (true, Some(stop), Some(state)) = (self.truncated(), &self.stop, SqlState::parse(TRUNCATED_STATE)) {
            stop.warnings().raise(state);
        }
    }
}
