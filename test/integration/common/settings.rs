//! The settings hook of the tests' plug-ins (RULES.md D36): one for every
//! kind of plug-in, so each declares and reads its settings alike. Split
//! from common.rs (RULES.md R-SIZE-2).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use turborel::settings::{Configurable, Definition, Effect, Kind, Settings};
use turborel::value::{Decimal, Value};

/// The whole number `n`, as a setting's value.
pub fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A plug-in's hook: declares the setting it is named, of 1 to 9, 1 by
/// default, taking effect as it is told, and keeps the value it is told,
/// -1 until told. One of `Effect::Start` keeps the first value, recorded
/// as the one it started with; any other, each value.
#[derive(Debug, Clone)]
pub struct Noted {
    setting: &'static str,
    effect: Effect,
    told: Arc<AtomicI64>,
}

impl Noted {
    /// A hook declaring `setting`, for each statement.
    pub fn new(setting: &'static str) -> Noted {
        Noted::taking(setting, Effect::Statement)
    }

    /// A hook declaring `setting`, taking effect as `effect` says.
    pub fn taking(setting: &'static str, effect: Effect) -> Noted {
        Noted { setting, effect, told: Arc::new(AtomicI64::new(-1)) }
    }

    /// The value it was told last, -1 if none.
    pub fn told(&self) -> i64 {
        self.told.load(Ordering::SeqCst)
    }
}

impl Configurable for Noted {
    fn settings(&self) -> Vec<Definition> {
        vec![Definition {
            name: self.setting.to_owned(),
            kind: Kind::Integer { min: 1, max: 9 },
            default: whole(1),
            per_session: false,
            node: false,
            effect: self.effect,
            doc: "How deep.".to_owned(),
        }]
    }
    fn configure(&self, settings: &Settings) {
        let Some(key) = settings.key(self.setting) else { return };
        if self.effect == Effect::Start && self.told() != -1 {
            return;
        }
        if self.effect == Effect::Start {
            settings.mark_started(key, None);
        }
        self.told.store(i64::try_from(settings.integer(key)).unwrap(), Ordering::SeqCst);
    }
}
