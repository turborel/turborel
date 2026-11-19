//! \[DESIGN\] Connection settings as text (RULES.md D202): `Options`, a
//! layer of settings each named by one of `KEYS`, as a program, a URI, a
//! service's section or the environment gives them. The layers are laid
//! one over another, each setting taken from the first that has it, then
//! read into `Settings`.

use std::collections::BTreeMap;

use crate::connection::Result;

use super::invalid;

/// The settings' names, as the program, a URI's query and a service file
/// write them.
pub const HOST: &str = "host";
/// See `HOST`.
pub const PORT: &str = "port";
/// See `HOST`.
pub const USER: &str = "user";
/// See `HOST`.
pub const PASSWORD: &str = "password";
/// See `HOST`.
pub const TLS_MODE: &str = "tls_mode";
/// See `HOST`.
pub const CHANNEL_BINDING: &str = "channel_binding";
/// See `HOST`.
pub const APPLICATION_NAME: &str = "application_name";
/// See `HOST`.
pub const CONNECT_TIMEOUT: &str = "connect_timeout";
/// See `HOST`.
pub const SERVICE: &str = "service";
/// See `HOST`.
pub const SERVICE_FILE: &str = "service_file";
/// See `HOST`.
pub const PASSFILE: &str = "passfile";
/// See `HOST`.
pub const TARGET: &str = "target";
/// See `HOST`.
pub const LOAD_BALANCE: &str = "load_balance";
/// See `HOST`.
pub const TLS_ROOTS: &str = "tls_roots";
/// See `HOST`.
pub const TLS_SYSTEM_ROOTS: &str = "tls_system_roots";
/// See `HOST`.
pub const TLS_CERTIFICATE: &str = "tls_certificate";
/// See `HOST`.
pub const TLS_KEY: &str = "tls_key";
/// See `HOST`.
pub const SERVER_NAME: &str = "server_name";

/// Every setting's name.
pub const KEYS: [&str; 18] = [
    HOST,
    PORT,
    USER,
    PASSWORD,
    TLS_MODE,
    CHANNEL_BINDING,
    APPLICATION_NAME,
    CONNECT_TIMEOUT,
    SERVICE,
    SERVICE_FILE,
    PASSFILE,
    TARGET,
    LOAD_BALANCE,
    TLS_ROOTS,
    TLS_SYSTEM_ROOTS,
    TLS_CERTIFICATE,
    TLS_KEY,
    SERVER_NAME,
];

/// The environment's variables, each the setting it gives.
pub const ENVIRONMENT: [(&str, &str); 10] = [
    ("TURBOREL_HOST", HOST),
    ("TURBOREL_PORT", PORT),
    ("TURBOREL_USER", USER),
    ("TURBOREL_PASSWORD", PASSWORD),
    ("TURBOREL_TLS_MODE", TLS_MODE),
    ("TURBOREL_APPLICATION_NAME", APPLICATION_NAME),
    ("TURBOREL_CONNECT_TIMEOUT", CONNECT_TIMEOUT),
    ("TURBOREL_SERVICE", SERVICE),
    ("TURBOREL_SERVICE_FILE", SERVICE_FILE),
    ("TURBOREL_PASSFILE", PASSFILE),
];

/// A layer of connection settings, each by its name.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<&'static str, String>,
}

impl std::fmt::Debug for Options {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = self.values.iter().map(|(k, v)| (*k, if *k == PASSWORD { "..." } else { v.as_str() }));
        f.debug_map().entries(shown).finish()
    }
}

impl Options {
    /// No settings.
    pub fn new() -> Options {
        Options::default()
    }

    /// Sets `key` to `value`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if `key` names no setting.
    pub fn set(&mut self, key: &str, value: &str) -> Result<&mut Options> {
        let known = KEYS.iter().find(|k| **k == key).ok_or_else(|| invalid(&format!("[DESIGN] {key} is no connection setting")))?;
        self.values.insert(known, value.to_owned());
        Ok(self)
    }

    /// The value of `key`, if set.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Whether none is set.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// These settings, laid over `lower`: each of `lower`'s kept where
    /// these have none.
    #[must_use]
    pub fn over(&self, lower: &Options) -> Options {
        let mut values = lower.values.clone();
        values.extend(self.values.iter().map(|(k, v)| (*k, v.clone())));
        Options { values }
    }

    /// The settings the environment gives, `variable` reading each of
    /// `ENVIRONMENT`'s variables: an empty one gives none.
    pub fn environment(variable: impl Fn(&str) -> Option<String>) -> Options {
        let mut values = BTreeMap::new();
        for (name, key) in ENVIRONMENT {
            if let Some(value) = variable(name).filter(|v| !v.is_empty()) {
                values.insert(key, value);
            }
        }
        Options { values }
    }
}
