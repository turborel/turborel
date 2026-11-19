//! \[DESIGN\] Connecting as `Settings` say, to one of several servers
//! (RULES.md D201): each host tried in the order given, or in an order
//! chosen at random for each connection under `LoadBalance::Random`, and
//! each address a host's name resolves to in turn, every step within the
//! connect timeout. A server reached, logged in to and of the role the
//! settings ask (`role.rs`) is kept; another is left and the next tried.
//! No server kept, the failure says why each was not.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::{SocketAddr, ToSocketAddrs};

use turborel::error::{Code, SqlError, SqlState};
use turborel_protocol::Failure;

use crate::agent::Connected;
#[cfg(not(feature = "scram"))]
use crate::connection::Password;
use crate::connection::{Mechanism, Result, Trust, failed, in_the_clear};
use crate::mechanism::Certificate;
use crate::role::{Role, role};
#[cfg(not(feature = "tls"))]
use crate::settings::invalid;
use crate::settings::{ChannelBinding, Host, LoadBalance, Settings, TargetRole, TlsMode};
use crate::target::{Target, log_in};
use crate::timeouts::{Timeouts, io_failed};

/// \[DESIGN\] The multiplier of the generator that orders hosts at random
/// (Knuth's MMIX linear congruential generator).
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
/// \[DESIGN\] Its increment (MMIX's).
const INCREMENT: u64 = 1_442_695_040_888_963_407;
/// \[DESIGN\] The high bits of its state taken for each draw, the low
/// bits of a linear congruential generator being the weakest.
const DRAW_SHIFT: u32 = 33;

/// A seed of the system's randomness, by the standard library alone: a
/// hasher of random keys, finished over nothing.
fn seed() -> u64 {
    RandomState::new().build_hasher().finish()
}

/// `items` in an order drawn from `seed` (Fisher and Yates, as Durstenfeld
/// gives it): each order as likely, for a seed drawn at random.
pub(crate) fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed;
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        let bound = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
        let drawn = (state >> DRAW_SHIFT).checked_rem(bound).unwrap_or(0);
        items.swap(i, usize::try_from(drawn).unwrap_or(0));
    }
}

/// What proves the user, as the settings say.
enum Proof {
    Trust(Trust),
    #[cfg(not(feature = "scram"))]
    Password(Password),
    Certificate(Certificate),
    #[cfg(feature = "scram")]
    Scram(Box<crate::scram::Scram>),
}

impl Proof {
    /// Whether it binds its exchange to a TLS channel: SCRAM alone.
    fn binds(&self) -> bool {
        match self {
            Proof::Trust(_) | Proof::Certificate(_) => false,
            #[cfg(not(feature = "scram"))]
            Proof::Password(_) => false,
            #[cfg(feature = "scram")]
            Proof::Scram(_) => true,
        }
    }

    fn mechanism(&mut self) -> &mut dyn Mechanism {
        match self {
            Proof::Trust(m) => m,
            #[cfg(not(feature = "scram"))]
            Proof::Password(m) => m,
            Proof::Certificate(m) => m,
            #[cfg(feature = "scram")]
            Proof::Scram(m) => m.as_mut(),
        }
    }
}

/// Why the server at `at` was not kept: `failure`.
fn why(at: &str, failure: &Failure) -> String {
    format!("{at}: {}", failure.message)
}

impl Settings {
    /// A connection to the first server, of the hosts in the order the
    /// settings say, that can be reached, logs the user in and is of the
    /// role asked.
    ///
    /// # Errors
    ///
    /// The one server's failure, if there was one; else SQLSTATE 08001,
    /// saying why each was not kept, of the last one's SQLSTATE where
    /// that was a refusal of the user (class 28). SQLSTATE 28000 before
    /// any connection, for a channel binding required by a mechanism that
    /// binds none.
    pub fn connect(&self) -> Result<Connected> {
        let mut hosts = self.hosts.clone();
        if self.load_balance == LoadBalance::Random {
            shuffle(&mut hosts, seed());
        }
        let tls = self.tls_config()?;
        let mut tried: Vec<(String, Failure)> = Vec::new();
        for host in &hosts {
            for (at, target) in self.targets(host, tls.as_ref()) {
                match target.and_then(|t| self.attempt(host, &t)) {
                    Ok(connection) => return Ok(connection),
                    Err(failure) => tried.push((at, failure)),
                }
            }
        }
        Err(none_kept(tried))
    }

    /// The TLS the settings' mode makes, if any.
    #[cfg(feature = "tls")]
    fn tls_config(&self) -> Result<Option<std::sync::Arc<rustls::ClientConfig>>> {
        crate::tls::modes::config(self.tls_mode, &self.tls)
    }

    /// None: without the feature `tls`, only `disable` and `prefer`.
    #[cfg(not(feature = "tls"))]
    fn tls_config(&self) -> Result<Option<()>> {
        match self.tls_mode {
            TlsMode::Disable | TlsMode::Prefer => Ok(None),
            TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => {
                Err(invalid("[DESIGN] TLS is used only by a client built with the feature `tls`"))
            }
        }
    }

    /// The targets of `host`, each named: its local socket, or each
    /// address its name resolves to, over TLS as `tls` says.
    #[cfg(feature = "tls")]
    fn targets(&self, host: &Host, tls: Option<&std::sync::Arc<rustls::ClientConfig>>) -> Vec<(String, Result<Target>)> {
        let Host::Tcp(name, port) = host else { return Settings::local(host) };
        let server = self.tls.server_name.clone().unwrap_or_else(|| name.clone());
        let secured = |a: SocketAddr| tls.map_or(Target::Tcp(a), |config| Target::Tls(a, server.clone(), std::sync::Arc::clone(config)));
        addresses(name, *port).into_iter().map(|(at, a)| (at, a.map(secured))).collect()
    }

    /// The targets of `host`, each named, over plain TCP.
    #[cfg(not(feature = "tls"))]
    fn targets(&self, host: &Host, _: Option<&()>) -> Vec<(String, Result<Target>)> {
        let Host::Tcp(name, port) = host else { return Settings::local(host) };
        addresses(name, *port).into_iter().map(|(at, a)| (at, a.map(Target::Tcp))).collect()
    }

    /// The target of a local `host`.
    fn local(host: &Host) -> Vec<(String, Result<Target>)> {
        let Host::Local(path) = host else { return Vec::new() };
        vec![(path.display().to_string(), Target::local(path))]
    }

    /// What proves the user at `host` over `target`.
    fn proof(&self, host: &Host, target: &Target) -> Result<Proof> {
        let bound = self.channel_binding == ChannelBinding::Require;
        let password = self.password_for(host);
        let proof = match password {
            #[cfg(feature = "scram")]
            Some(p) => {
                Proof::Scram(Box::new(crate::scram::Scram::new(&p).channel_binding(self.channel_binding).with_cache(&self.key_cache)))
            }
            #[cfg(not(feature = "scram"))]
            Some(p) => Proof::Password(Password(p)),
            None => {
                if self.tls.certificate.is_some() {
                    Proof::Certificate(Certificate)
                } else {
                    Proof::Trust(Trust)
                }
            }
        };
        if bound && !(proof.binds() && target.is_tls()) {
            let e = SqlError::new(Code::Connection, "[DESIGN] channel binding is required: SCRAM over TLS alone binds one")
                .with_state(SqlState::INVALID_AUTHORIZATION);
            return Err(Failure::from(&e));
        }
        Ok(proof)
    }

    /// A connection to `host` over `target`, logged in and of the role
    /// asked; under `TlsMode::Prefer`, over plain TCP if TLS cannot be
    /// begun.
    fn attempt(&self, host: &Host, target: &Target) -> Result<Connected> {
        let timeouts = Timeouts { connect: self.connect_timeout, read: None };
        let (target, opened) = match self.tls_mode {
            TlsMode::Prefer => target.opened_preferring(timeouts.connect)?,
            TlsMode::Disable | TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => {
                (target.clone(), target.opened(timeouts.connect)?)
            }
        };
        let mut proof = self.proof(host, &target)?;
        if target.is_plain_tcp() {
            in_the_clear(proof.mechanism())?;
        }
        let mut connection = log_in(&target, opened, (&self.user, proof.mechanism()), timeouts)?;
        self.of_role(&mut connection)?;
        Ok(connection)
    }

    /// Refuses `connection`, ended, if its server is not of the role the
    /// settings ask.
    fn of_role(&self, connection: &mut Connected) -> Result<()> {
        let wanted = match self.target {
            TargetRole::Any => return Ok(()),
            TargetRole::ReadWrite => Role::ReadWrite,
            TargetRole::ReadOnly => Role::ReadOnly,
        };
        let found = role(connection)?;
        if found == wanted {
            return Ok(());
        }
        Err(failed(&format!("[DESIGN] the server is {}, and {} is asked", found.describe(), wanted.describe())))
    }
}

/// Each address `name` resolves to at `port`, named; or why none.
fn addresses(name: &str, port: u16) -> Vec<(String, Result<SocketAddr>)> {
    match (name, port).to_socket_addrs() {
        Ok(found) => found.map(|a| (format!("{name} ({a})"), Ok(a))).collect(),
        Err(e) => vec![(format!("{name}:{port}"), Err(io_failed("resolve the name", &e)))],
    }
}

/// The failure of a connection none of `tried` gave.
fn none_kept(mut tried: Vec<(String, Failure)>) -> Failure {
    if tried.len() == 1
        && let Some((_, failure)) = tried.pop()
    {
        return failure;
    }
    let refused = tried.last().is_some_and(|(_, f)| f.sqlstate.starts_with("28"));
    let state = if refused { SqlState::INVALID_AUTHORIZATION } else { SqlState::UNABLE_TO_CONNECT };
    let reasons: Vec<String> = tried.iter().map(|(at, f)| why(at, f)).collect();
    let msg = if reasons.is_empty() {
        "[DESIGN] no server is named".to_owned()
    } else {
        format!("[DESIGN] no server was kept: {}", reasons.join("; "))
    };
    Failure::from(&SqlError::new(Code::Connection, msg).with_state(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D201: hosts ordered at random are each
    /// host once, in an order the seed decides, the same for the same
    /// seed; different seeds give different orders; one host or none
    /// stays as it is.
    #[test]
    fn hosts_are_shuffled_by_their_seed() {
        let hosts: Vec<u32> = (0..8).collect();
        let order = |seed: u64| {
            let mut h = hosts.clone();
            shuffle(&mut h, seed);
            h
        };
        let mut sorted = order(7);
        sorted.sort_unstable();
        assert_eq!(sorted, hosts);
        assert_eq!(order(7), order(7));
        let orders: std::collections::BTreeSet<Vec<u32>> = (0..16).map(order).collect();
        assert!(orders.len() > 8, "{orders:?}");
        let mut one = [1_u8];
        shuffle(&mut one, 3);
        shuffle(&mut [0_u8; 0], 3);
        assert_eq!(one, [1_u8]);
        assert_ne!(seed(), seed(), "each seed the system's own");
    }

    /// Verifies: [DESIGN] RULES.md D201: of one server tried, its own
    /// failure is given; of several, SQLSTATE 08001 names each and why,
    /// or 28000 where the last refused the user; of none, 08001.
    #[test]
    fn no_server_kept_says_why_each_was_not() {
        let lost = failed("[DESIGN] cannot connect");
        let refused = Failure::from(&SqlError::new(Code::Connection, "no").with_state(SqlState::INVALID_AUTHORIZATION));
        assert_eq!(none_kept(vec![("a".to_owned(), refused.clone())]).sqlstate, "28000");
        let both = none_kept(vec![("a".to_owned(), refused.clone()), ("b".to_owned(), lost.clone())]);
        assert_eq!(both.sqlstate, "08001");
        assert!(both.message.contains("a: no") && both.message.contains("b: cannot connect"), "{}", both.message);
        assert_eq!(none_kept(vec![("b".to_owned(), lost), ("a".to_owned(), refused)]).sqlstate, "28000");
        assert_eq!(none_kept(Vec::new()).sqlstate, "08001");
    }
}
