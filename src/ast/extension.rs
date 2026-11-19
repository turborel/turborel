//! \[EXTENSION\] Abstract syntax of the statements of extensions from
//! other parties (RULES.md D168): CREATE EXTENSION and ALTER EXTENSION
//! ... UPDATE. DROP EXTENSION is a `DropObject`.

/// \[EXTENSION\] A statement that creates or updates an extension.
#[derive(Debug, Clone)]
pub enum ExtensionStatement {
    /// CREATE EXTENSION.
    Create(ExtensionDefinition),
    /// ALTER EXTENSION ... UPDATE.
    Update(ExtensionDefinition),
}

impl ExtensionStatement {
    /// Its definition.
    pub fn definition(&self) -> &ExtensionDefinition {
        match self {
            ExtensionStatement::Create(d) | ExtensionStatement::Update(d) => d,
        }
    }
}

/// \[EXTENSION\] `<extension definition>` ::= CREATE EXTENSION
/// `<identifier>` \[ VERSION `<character string literal>` \] \[ FROM
/// `<character string literal>` \] \[ `<extension as read>` \]; and ALTER
/// EXTENSION `<identifier>` UPDATE \[ TO `<character string literal>` \]
/// \[ FROM `<character string literal>` \] \[ `<extension as read>` \].
#[derive(Debug, Clone)]
pub struct ExtensionDefinition {
    /// Its name.
    pub name: String,
    /// Where its name begins in the statement.
    pub at: usize,
    /// VERSION or TO: a version, or a range of them.
    pub version: Option<String>,
    /// FROM: a package file or a directory.
    pub from: Option<String>,
    /// What the database read of it, which the text it logs holds.
    pub read: Option<ExtensionRead>,
}

/// \[EXTENSION\] `<extension as read>` ::= MANIFEST `<character string
/// literal>` DIRECTORY `<character string literal>` \[ SIGNER ... \] \[
/// DIGEST ... \] \[ AS DEPENDENCY \]: what CREATE or
/// ALTER EXTENSION read, written by the database into the text it logs,
/// so recovery and a replica need nothing but the directory's program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionRead {
    /// The manifest's text.
    pub manifest: String,
    /// The directory, absolute.
    pub directory: String,
    /// The key that signed its package, if one did.
    pub signer: Option<String>,
    /// Its package's digest, if it came of one.
    pub digest: Option<String>,
    /// Created only as another required it.
    pub dependency: bool,
}
