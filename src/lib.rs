//! turborel: a relational database system. Its language is ANSI X3.135-1986,
//! "Database Language SQL" (adopted as FIPS PUB 127), as extended by
//! ISO/IEC 9075:1992, one language where the newer wins (D32); later ISO SQL
//! standards will extend it the same way.
//!
//! # Citation convention
//!
//! Many SQL standards and dialects exist (ISO/IEC 9075 editions, vendor
//! extensions), and their clause numbers differ. So every reference to a
//! standard names the document:
//!
//! ```text
//! [X3.135-1986 §8.6 GR2]           ANSI X3.135-1986, clause 8.6, General Rule 2
//! [ISO/IEC 9075:1992 §11.19 SR13]  ISO/IEC 9075:1992, clause 11.19, Syntax Rule 13
//! ```
//!
//! * `F` is a Format (the BNF), `SR` a Syntax Rule, `GR` a General Rule.
//!   Sub-items are written as printed: `GR3b vii`. Several rules of one
//!   clause share a bracket: `[X3.135-1986 §5.8 GR1, GR2]`.
//! * "SQL-86" in prose means ANSI X3.135-1986, "SQL-92" ISO/IEC 9075:1992.
//! * "implementor-defined" beside a citation means the standard leaves the
//!   choice to the implementation; the comment states the choice made here.
//! * `[EXTENSION]` marks a facility neither standard defines. Both allow
//!   them: "A conforming implementation may provide additional facilities"
//!   [X3.135-1986 §3.4] [ISO/IEC 9075:1992 §23.3].
//! * `[DESIGN]` marks internals the standards do not govern (storage, plans,
//!   optimization). Where an idea comes from the literature, it is named.
//!
//! The same form is used in error messages, so a user can look a rule up.

pub mod ast;
pub mod binder;
pub mod bytes;
pub mod catalog;
pub mod clock;
pub mod db;
pub mod diagnostics;
pub mod disk;
pub mod engine;
pub mod error;
pub mod exec;
pub mod files;
pub mod functions;
pub mod hash;
pub mod index;
pub mod lexer;
pub mod log;
pub mod logical;
pub mod memory;
pub mod optimizer;
pub mod params;
pub mod parser;
pub mod physical;
pub mod settings;
pub mod spill;
pub mod stats;
pub mod storage;
mod sync;
pub mod system;
pub mod threads;
pub mod txn;
pub mod value;

/// The manual's chapter on embedding the database (`doc/client/`), whose
/// examples run as its doc tests (RULES.md R-DOC-8).
#[cfg(doctest)]
#[doc = include_str!("../doc/client/03-embedding.md")]
pub struct EmbeddedLibraryChapter;

/// The version of turborel, every crate of it at once (VERSIONING.md).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// This release's codename (VERSIONING.md): its own, if it marks a
/// milestone, else the last named release's of its major version; "alpha"
/// before 1.0.0.
pub const CODENAME: &str = "alpha";

/// The product, its version and its codename, as "turborel 0.0.0
/// (alpha)": what a server tells its clients, and its programs print.
pub fn product() -> String {
    format!("turborel {VERSION} ({CODENAME})")
}

#[cfg(test)]
mod version_tests {
    /// Verifies: [DESIGN] VERSIONING.md: before 1.0.0, every version is
    /// alpha's; the product names its version and codename.
    #[test]
    fn a_version_has_its_codename() {
        let major = super::VERSION.split('.').next();
        assert_eq!((major, super::CODENAME), (Some("0"), "alpha"));
        assert_eq!(super::product(), format!("turborel {} (alpha)", super::VERSION));
    }
}
