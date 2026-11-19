//! \[EXTENSION\] The functions of advisory locks (RULES.md D187): of a
//! session's hold, to its end or until unlocked, and of a transaction's,
//! to its end; exclusive or shared; waiting, or tried and given up at
//! once. A key is a BIGINT, two INTEGERs or a character string. Each gives
//! a BOOLEAN: true, of a lock taken or unlocked; false, of a try that
//! found the key held, or an unlock of a key the session does not hold.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{CallContext, ScalarFunction};
use crate::error::{Code, Result, SqlError};
use crate::txn::{AdvisoryKey, Asker, LockWait, Waiter, Waits};
use crate::value::{DataType, Value};

/// What a function of advisory locks does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    /// Takes a lock, waiting while another session holds it.
    Lock,
    /// Takes a lock if no other session holds it.
    Try,
    /// Unlocks one of the session's own holds.
    Unlock,
    /// Unlocks every one of the session's own holds.
    UnlockAll,
}

/// \[EXTENSION\] A function of advisory locks.
#[derive(Debug, Clone, Copy)]
struct Advisory {
    name: &'static str,
    act: Act,
    /// The transaction's hold, else the session's.
    xact: bool,
    exclusive: bool,
}

/// \[EXTENSION\] The functions of advisory locks, each by its name.
const FUNCTIONS: [Advisory; 11] = [
    Advisory { name: "ADVISORY_LOCK", act: Act::Lock, xact: false, exclusive: true },
    Advisory { name: "ADVISORY_LOCK_SHARED", act: Act::Lock, xact: false, exclusive: false },
    Advisory { name: "TRY_ADVISORY_LOCK", act: Act::Try, xact: false, exclusive: true },
    Advisory { name: "TRY_ADVISORY_LOCK_SHARED", act: Act::Try, xact: false, exclusive: false },
    Advisory { name: "ADVISORY_UNLOCK", act: Act::Unlock, xact: false, exclusive: true },
    Advisory { name: "ADVISORY_UNLOCK_SHARED", act: Act::Unlock, xact: false, exclusive: false },
    Advisory { name: "ADVISORY_UNLOCK_ALL", act: Act::UnlockAll, xact: false, exclusive: true },
    Advisory { name: "ADVISORY_XACT_LOCK", act: Act::Lock, xact: true, exclusive: true },
    Advisory { name: "ADVISORY_XACT_LOCK_SHARED", act: Act::Lock, xact: true, exclusive: false },
    Advisory { name: "TRY_ADVISORY_XACT_LOCK", act: Act::Try, xact: true, exclusive: true },
    Advisory { name: "TRY_ADVISORY_XACT_LOCK_SHARED", act: Act::Try, xact: true, exclusive: false },
];

/// \[EXTENSION\] The names a dialect's text may call these by, each the
/// name it stands for with the prefix `PG_` (RULES.md D187).
pub const ADVISORY_ALIASES: [(&str, &str); 11] = [
    ("PG_ADVISORY_LOCK", "ADVISORY_LOCK"),
    ("PG_ADVISORY_LOCK_SHARED", "ADVISORY_LOCK_SHARED"),
    ("PG_TRY_ADVISORY_LOCK", "TRY_ADVISORY_LOCK"),
    ("PG_TRY_ADVISORY_LOCK_SHARED", "TRY_ADVISORY_LOCK_SHARED"),
    ("PG_ADVISORY_UNLOCK", "ADVISORY_UNLOCK"),
    ("PG_ADVISORY_UNLOCK_SHARED", "ADVISORY_UNLOCK_SHARED"),
    ("PG_ADVISORY_UNLOCK_ALL", "ADVISORY_UNLOCK_ALL"),
    ("PG_ADVISORY_XACT_LOCK", "ADVISORY_XACT_LOCK"),
    ("PG_ADVISORY_XACT_LOCK_SHARED", "ADVISORY_XACT_LOCK_SHARED"),
    ("PG_TRY_ADVISORY_XACT_LOCK", "TRY_ADVISORY_XACT_LOCK"),
    ("PG_TRY_ADVISORY_XACT_LOCK_SHARED", "TRY_ADVISORY_XACT_LOCK_SHARED"),
];

/// The functions of advisory locks.
pub(super) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    FUNCTIONS.into_iter().map(|f| -> Arc<dyn ScalarFunction> { Arc::new(f) }).collect()
}

/// The refusal of a call of `name` of other arguments.
fn arguments(name: &str) -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        format!("[EXTENSION] {name} takes a key: an exact number of scale 0, two of them, or a character string (RULES.md D187)"),
    )
}

/// Whether `t` is an exact number of scale 0.
fn whole(t: &DataType) -> bool {
    matches!(t, DataType::Exact { scale: 0, .. })
}

/// The key `args` give, none if one is null.
fn key_of(name: &str, args: &[Value]) -> Result<Option<AdvisoryKey>> {
    let number = |v: &Value| if let Value::Exact(d) = v { (d.scale == 0).then_some(d.units) } else { None };
    if args.iter().any(Value::is_null) {
        return Ok(None);
    }
    let key = match args {
        [Value::Char(s)] => Some(AdvisoryKey::Name(s.clone())),
        [one] => number(one).and_then(|n| i64::try_from(n).ok()).map(AdvisoryKey::Int),
        [a, b] => number(a).zip(number(b)).and_then(|(a, b)| Some(AdvisoryKey::Pair(i32::try_from(a).ok()?, i32::try_from(b).ok()?))),
        _ => None,
    };
    key.map(Some).ok_or_else(|| {
        SqlError::new(Code::Data, format!("[EXTENSION] the key of {name} is out of the range of its type (RULES.md D187)"))
            .with_state(crate::error::SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
    })
}

impl ScalarFunction for Advisory {
    fn name(&self) -> &str {
        self.name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fits = match (self.act, args) {
            (Act::UnlockAll, []) => true,
            (Act::Lock | Act::Try | Act::Unlock, [one]) => whole(one) || one.is_character(),
            (Act::Lock | Act::Try | Act::Unlock, [a, b]) => whole(a) && whole(b),
            (Act::Lock | Act::Try | Act::Unlock | Act::UnlockAll, _) => false,
        };
        if fits { Ok(DataType::Boolean) } else { Err(arguments(self.name)) }
    }

    fn call(&self, _args: &[Value]) -> Result<Value> {
        Err(SqlError::new(Code::Unsupported, format!("[EXTENSION] {} runs only in a statement of a session (RULES.md D187)", self.name)))
    }

    fn uses_context(&self, _arity: usize) -> bool {
        true
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        let Some((st, tx)) = cx.transaction() else { return self.call(args) };
        let shared = Arc::clone(&st.tx);
        let stop = shared.stop_of(tx).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the transaction of the call is not open"))?;
        let session = stop.session;
        let space = shared.advisory();
        if self.act == Act::UnlockAll {
            space.unlock_all(session);
            return Ok(Value::Bool(true));
        }
        let Some(key) = key_of(self.name, args)? else { return Ok(Value::Null) };
        if self.act == Act::Unlock {
            return Ok(Value::Bool(space.unlock(session, &key, self.exclusive)));
        }
        let lock_wait = if stop.waits.load(Ordering::SeqCst) { LockWait::Wait } else { LockWait::NoWait };
        let running = Arc::new(std::sync::Mutex::new(None));
        let waiter = Waiter { id: tx, stop: Arc::clone(&stop), lock_wait, shared: Arc::clone(&shared), running };
        let asker = Asker { waiter, session, xact: self.xact, exclusive: self.exclusive };
        let waits = if self.act == Act::Try { Waits::Skip } else { Waits::Setting };
        Ok(Value::Bool(space.lock(&asker, &key, waits)?))
    }

    fn deterministic(&self) -> bool {
        false
    }
}
