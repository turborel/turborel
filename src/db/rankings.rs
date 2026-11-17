//! \[EXTENSION\] The rankings of full-text search (RULES.md D87): BM25,
//! built in, and those registered (`Database::register_ranking`), which an
//! index names by FULLTEXT.RANKING and Rank scores by.
//!
//! The registry is shared, not copied with each change of the database's
//! state, so a full-text index method attached before a ranking is
//! registered sees it at the next CREATE or ALTER INDEX.

use std::sync::{Arc, PoisonError, RwLock};

use super::Database;
use crate::diagnostics::Extension;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::rank::{Ranking, Rankings, built_in, named};
use crate::lexer::authorization_identifier;

/// The rankings registered, beside the built-in.
#[derive(Clone, Default)]
pub(crate) struct SharedRankings(Arc<RwLock<Vec<Arc<dyn Ranking>>>>);

impl SharedRankings {
    /// Every ranking, the built-in first, as they are registered when it is
    /// called: what a full-text index method reads (`with_rankings`).
    pub(crate) fn closure(&self) -> Rankings {
        let shared = Arc::clone(&self.0);
        let builtin = built_in();
        Arc::new(move || {
            let mut all = builtin();
            all.extend(shared.read().unwrap_or_else(PoisonError::into_inner).iter().cloned());
            all
        })
    }

    /// The ranking named `name`, in any case.
    pub(crate) fn named(&self, name: &str) -> Option<Arc<dyn Ranking>> {
        named(&self.closure(), name)
    }

    /// Adds `ranking`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a ranking of its name is built in or registered.
    fn add(&self, ranking: Arc<dyn Ranking>) -> Result<()> {
        if self.named(ranking.name()).is_some() {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] a ranking {} is registered already", ranking.name())));
        }
        self.0.write().unwrap_or_else(PoisonError::into_inner).push(ranking);
        Ok(())
    }
}

impl Database {
    /// \[EXTENSION\] Registers a ranking of full-text search, which an
    /// index names by its option FULLTEXT.RANKING and Rank then scores its
    /// column's documents by (RULES.md D87). Its contract is `Ranking`'s.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is no regular identifier in upper case, or
    /// a ranking of its name is built in or registered already.
    pub fn register_ranking(&self, ranking: Box<dyn Ranking>) -> Result<()> {
        let ranking: Arc<dyn Ranking> = Arc::from(ranking);
        let name = ranking.name().to_owned();
        if authorization_identifier(&name).ok().as_deref() != Some(name.as_str()) {
            let done =
                Err(SqlError::new(Code::Api, format!("[DESIGN] a ranking is named by a regular identifier in upper case, not {name}")));
            return self.registered((Extension::Ranking, &name), done);
        }
        let done = self.snapshot().and_then(|st| st.rankings.add(ranking));
        self.registered((Extension::Ranking, &name), done)
    }
}
