//! \[EXTENSION\] Users and the database's owner (RULES.md D43). Which
//! `<identifier>`s are valid `<authorization identifier>`s is
//! implementation-defined [ISO/IEC 9075:1992 §5.4; Annex B, item 41]:
//! here any is, for a session the host program starts; a user is one a
//! server's clients may prove they are, by a verifier the database keeps.
//!
//! A verifier is opaque to the database: a `Credentials` plug-in makes it
//! of a password, and the authenticator that made it checks a password
//! against it. Users and the owner are definitions, logged and kept with
//! the others, made outside any transaction. The owner, a user, is the one
//! who manages the users in SQL (RULES.md D43); the host program's Rust
//! API manages them unrestricted.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

use super::{Completion, Database, Session, State};
use crate::ast::UserStatement;
use crate::diagnostics::{Event, Extension};
use crate::error::{At, Code, Result, SqlError, fail};
use crate::lexer::MAX_IDENTIFIER_LENGTH;
use crate::log::record::Change;

/// \[EXTENSION\] What makes a user's verifier of a password: an extension
/// point (RULES.md D43).
///
/// Contract (RULES.md R-SAFE-3): `verifier` returns bytes from which no
/// one can find the password sooner than by guessing it, made slow to
/// guess (D26); the authenticator of the same scheme checks a password
/// against them. It may take long, and never calls back into the
/// database (R-ERR-4).
pub trait Credentials: Send + Sync {
    /// Its name, for diagnostics.
    fn name(&self) -> &'static str;
    /// The verifier of `password`, for the user `user`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot make one, as when the system gives it no
    /// randomness.
    fn verifier(&self, user: &str, password: &str) -> Result<Vec<u8>>;
}

/// The users, their verifiers, and the owner: part of the definitions,
/// copied with them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Users {
    owner: Option<String>,
    verifiers: Arc<BTreeMap<String, Vec<u8>>>,
}

impl Users {
    /// The owner, if the database has one.
    pub(crate) fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// Whether `name` is a user.
    pub(crate) fn exists(&self, name: &str) -> bool {
        self.verifiers.contains_key(name)
    }

    /// Every user and its verifier, in order of name.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&String, &Vec<u8>)> {
        self.verifiers.iter()
    }

    /// `name`'s verifier set, or with none, `name` dropped, as a log
    /// record says: checked when it was made.
    pub(crate) fn replay_user(&mut self, name: &str, verifier: Option<Vec<u8>>) {
        let verifiers = Arc::make_mut(&mut self.verifiers);
        match verifier {
            Some(v) => verifiers.insert(name.to_owned(), v),
            None => verifiers.remove(name),
        };
    }

    /// The owner set, as a log record says: checked when it was made.
    pub(crate) fn replay_owner(&mut self, name: &str) {
        self.owner = Some(name.to_owned());
    }
}

/// A user statement's Syntax Rule broken (SQLCODE -103).
fn refused(message: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {message}"))
}

/// What is done to a user.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Create,
    Change,
    Drop,
}

impl State {
    /// Does `act` to the user `name`, of `verifier`, and logs it.
    fn define_user(&mut self, name: &str, verifier: Option<&[u8]>, act: Act) -> Result<()> {
        let length = name.chars().count();
        if length == 0 || length > MAX_IDENTIFIER_LENGTH {
            return Err(refused(&format!("a user's name is from 1 to {MAX_IDENTIFIER_LENGTH} characters")));
        }
        let exists = self.users.exists(name);
        if act == Act::Create && exists {
            return Err(refused(&format!("a user {name} exists already")));
        }
        if act != Act::Create && !exists {
            return Err(refused(&format!("there is no user {name}")));
        }
        if act == Act::Drop && self.users.owner() == Some(name) {
            return Err(refused(&format!("{name} owns the database: give it another owner first")));
        }
        let kept = verifier.map(<[u8]>::to_vec);
        self.users.replay_user(name, kept.clone());
        self.log_definition(None, Change::User { name: name.to_owned(), verifier: kept })
    }

    /// Makes the user `name` the owner, and logs it.
    fn define_owner(&mut self, name: &str) -> Result<()> {
        if !self.users.exists(name) {
            return Err(refused(&format!("there is no user {name} to own the database")));
        }
        self.users.replay_owner(name);
        self.log_definition(None, Change::Owner { name: name.to_owned() })
    }
}

impl Database {
    /// Does `act` to the user `name`, and tells the diagnostics.
    /// The first user created, of a database with no owner, owns it, in the
    /// same change: so a database never has users and no owner.
    fn user(&self, name: &str, verifier: Option<&[u8]>, act: Act) -> Result<()> {
        let owns = self.change(|st| {
            st.define_user(name, verifier, act)?;
            let first = act == Act::Create && st.users.owner().is_none();
            if first {
                st.define_owner(name)?;
            }
            Ok(first)
        })?;
        self.emit(&Event::UserChanged { name: name.to_owned(), dropped: act == Act::Drop });
        if owns {
            self.emit(&Event::OwnerChanged { name: name.to_owned() });
        }
        Ok(())
    }

    /// \[EXTENSION\] Creates the user `name`, an authorization identifier as
    /// written, with `verifier`, which a `Credentials` plug-in made. The
    /// first user of a database with no owner becomes its owner.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `name` is empty, longer than an identifier may be,
    /// or a user already; the log's errors.
    pub fn create_user(&self, name: &str, verifier: &[u8]) -> Result<()> {
        self.user(name, Some(verifier), Act::Create)
    }

    /// \[EXTENSION\] Gives the user `name` the verifier `verifier`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such user; the log's errors.
    pub fn set_verifier(&self, name: &str, verifier: &[u8]) -> Result<()> {
        self.user(name, Some(verifier), Act::Change)
    }

    /// \[EXTENSION\] Drops the user `name`. What its authorization
    /// identifier owns stays: an identifier is a name, not a user.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such user, or it owns the database; the
    /// log's errors.
    pub fn drop_user(&self, name: &str) -> Result<()> {
        self.user(name, None, Act::Drop)
    }

    /// \[EXTENSION\] The verifier of the user `name`, if there is one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn verifier(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.snapshot()?.users.entries().find(|(n, _)| *n == name).map(|(_, v)| v.clone()))
    }

    /// \[EXTENSION\] Whether `name` is a user: so a server ends the
    /// sessions of a user once it is dropped (RULES.md D43).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn is_user(&self, name: &str) -> Result<bool> {
        Ok(self.snapshot()?.users.exists(name))
    }

    /// \[EXTENSION\] Every user, in order of name.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn users(&self) -> Result<Vec<String>> {
        Ok(self.snapshot()?.users.entries().map(|(n, _)| n.clone()).collect())
    }

    /// \[EXTENSION\] Makes the user `name` the database's owner.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such user; the log's errors.
    pub fn set_owner(&self, name: &str) -> Result<()> {
        self.change(|st| st.define_owner(name))?;
        self.emit(&Event::OwnerChanged { name: name.to_owned() });
        Ok(())
    }

    /// \[EXTENSION\] The database's owner, if it has one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn owner(&self) -> Result<Option<String>> {
        Ok(self.snapshot()?.users.owner().map(str::to_owned))
    }

    /// \[EXTENSION\] Sets what makes users' verifiers of the passwords SQL
    /// gives them (`CREATE USER ... PASSWORD`), in place of any set before.
    pub fn set_credentials(&self, credentials: Box<dyn Credentials>) {
        let name = credentials.name().to_owned();
        *self.credentials_slot().write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(credentials));
        self.emit(&Event::Registered { kind: Extension::Credentials, name });
    }

    /// What makes verifiers, if it was set.
    fn credentials(&self) -> Option<Arc<dyn Credentials>> {
        self.credentials_slot().read().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Session {
    /// \[EXTENSION\] Checks that `what`, a statement only the database's
    /// owner runs (RULES.md D36, D43), may run: outside a transaction, as a
    /// definition is made, by the owner.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 in a transaction; -104 if the session's identifier is
    /// not the owner's, or the database has none.
    pub(super) fn as_owner(&self, what: &str) -> Result<()> {
        if self.inner.borrow().tx.is_some() {
            return fail(Code::TransactionState, format!("[EXTENSION] {what} runs outside a transaction: commit or roll back first"));
        }
        if self.db.owner()?.as_deref() != Some(self.auth().as_str()) {
            return fail(Code::Privilege, format!("[EXTENSION] only the database's owner runs {what}, and {} is not it", self.auth()));
        }
        Ok(())
    }

    /// \[EXTENSION\] Runs the user statement `stmt`: by the database's
    /// owner only, outside a transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 in a transaction; -104 if the session's identifier is
    /// not the owner's; -910 if no `Credentials` plug-in is set to make a
    /// verifier; those of `Database::create_user`, `set_verifier`,
    /// `drop_user`.
    pub(super) fn user_statement(&self, stmt: &UserStatement) -> Result<Completion> {
        self.as_owner("a user statement")?;
        let verified = |name: &str, password: &str| {
            let credentials = self.db.credentials().ok_or_else(|| {
                SqlError::new(Code::Api, "[EXTENSION] no credentials plug-in is set to keep a password (Database::set_credentials)")
            })?;
            credentials.verifier(name, password)
        };
        match stmt {
            UserStatement::Create { name: (name, at), password } => self.db.create_user(name, &verified(name, password)?).or_at(*at)?,
            UserStatement::Alter { name: (name, at), password } => self.db.set_verifier(name, &verified(name, password)?).or_at(*at)?,
            UserStatement::Drop { name: (name, at) } => self.db.drop_user(name).or_at(*at)?,
        }
        Ok(Completion::Success)
    }
}
