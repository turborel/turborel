//! \[EXTENSION\] Auto-embedded columns (§12.4) and pipelines (§12.5) as
//! the statements that keep them, so that the database's own generated
//! columns (RULES.md D63) and triggers (D71) do the work.
//!
//! * An auto-embedded column is a generated column of EMBED of its
//!   source: `ALTER TABLE t ADD COLUMN c VECTOR(d, f, l) GENERATED ALWAYS
//!   AS (EMBED('M', body))`, computed in the writing statement, which is
//!   §12.4's SYNC.
//! * A pipeline reads `CREATE VECTOR PIPELINE p SOURCE s (key..., text)
//!   [CHUNK BY ...] EMBED USING m TARGET t (key..., chunk_id, chunk_text,
//!   embedding) [MODE SYNC | ASYNC]`. Its target table exists already;
//!   its plan fills it from the rows the source has, then keeps it in
//!   step by four row triggers. After an insert the new row's chunks are
//!   added, after a delete the old row's removed, and after an update of
//!   the text or a key the old row's are removed and the new row's added,
//!   triggers firing in the order they were made. CHUNK left out is
//!   CHUNK_TEXT's default, BY WORDS MAX 256.
//!
//! What the database lacks for the rest is a plan's `hooks`.

use super::{Embedders, ModelShape};
use crate::error::{Code, Result, SqlError};
use crate::vector::chunk::{Spec, quote};

/// \[EXTENSION\] When an embedding is computed (§12.4 R12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// In the writing statement.
    Sync,
    /// By a background queue, the vector null until then.
    Async,
}

/// \[EXTENSION\] What a plan needs that turborel does not yet do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// ASYNC: a background queue filling the vectors, its status in the
    /// catalog, and VECTOR_WAIT_FRESH (R12.6). The plan computes them
    /// as SYNC does until it exists.
    AsyncQueue,
    /// R12.5: a hidden column of each vector's model and version.
    ModelVersion,
    /// R12.3: the executor giving a model the texts of many rows at once.
    Batching,
}

/// \[EXTENSION\] The statements that keep an auto-embedded column or a
/// pipeline, in order, and what they still need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// SQL statements, run in order.
    pub statements: Vec<String>,
    /// What the database must still give.
    pub hooks: Vec<Hook>,
}

fn refused(detail: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {detail}"))
}

/// The VECTOR type of `shape`, as written.
fn type_text(shape: ModelShape) -> String {
    format!("VECTOR({}, {}, {})", shape.dims, shape.element.name(), shape.layout.name())
}

/// The hooks of `mode`.
fn hooks(mode: Mode) -> Vec<Hook> {
    let mut out = vec![Hook::ModelVersion, Hook::Batching];
    if mode == Mode::Async {
        out.insert(0, Hook::AsyncQueue);
    }
    out
}

/// The shape of the model `name`.
fn shape_of(models: &Embedders, name: &str) -> Result<ModelShape> {
    Ok(models.named(name)?.ok_or_else(|| refused(&format!("names no model {name}")))?.shape())
}

/// \[EXTENSION\] An auto-embedded column (§12.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoColumn {
    /// The table.
    pub table: String,
    /// The column made.
    pub column: String,
    /// The column of text it embeds.
    pub source: String,
    /// The model.
    pub model: String,
    /// When it is computed.
    pub mode: Mode,
}

impl AutoColumn {
    /// The statement adding it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no model `model` is registered.
    pub fn plan(&self, models: &Embedders) -> Result<Plan> {
        let shape = shape_of(models, &self.model)?;
        let statement = format!(
            "ALTER TABLE {} ADD COLUMN {} {} GENERATED ALWAYS AS (EMBED({}, {}))",
            self.table,
            self.column,
            type_text(shape),
            quote(&self.model.to_uppercase()),
            self.source
        );
        Ok(Plan { statements: vec![statement], hooks: hooks(self.mode) })
    }
}

/// \[EXTENSION\] A pipeline (§12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    /// Its name.
    pub name: String,
    /// The source table.
    pub source: String,
    /// The source's key columns.
    pub keys: Vec<String>,
    /// The source's column of text.
    pub text: String,
    /// How the text is chunked.
    pub chunk: Spec,
    /// The model.
    pub model: String,
    /// The target table.
    pub target: String,
    /// The target's columns: one a key, then chunk id, chunk text and
    /// embedding.
    pub columns: Vec<String>,
    /// When it embeds.
    pub mode: Mode,
}

/// The tokens of a statement: words, numbers, quoted strings kept quoted,
/// and parentheses and commas.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut token = String::from(c);
        if c == '\'' {
            while let Some(next) = chars.next() {
                token.push(next);
                if next == '\'' && chars.next_if_eq(&'\'').map(|q| token.push(q)).is_none() {
                    break;
                }
            }
        } else if c != '(' && c != ')' && c != ',' {
            token.extend(std::iter::from_fn(|| chars.next_if(|n| !n.is_whitespace() && !"(),'".contains(*n))));
            token = token.to_uppercase();
        }
        out.push(token);
    }
    out
}

/// A cursor over a statement's tokens.
struct Tokens {
    tokens: Vec<String>,
    at: usize,
}

impl Tokens {
    fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.at)?;
        self.at = self.at.saturating_add(1);
        Some(token)
    }
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }
    fn word(&mut self, word: &str) -> Result<()> {
        if self.next() == Some(word) { Ok(()) } else { Err(refused(&format!("a pipeline has {word} here"))) }
    }
    fn name(&mut self) -> Result<String> {
        let name = self.next().filter(|t| t.chars().next().is_some_and(char::is_alphabetic)).ok_or_else(|| refused("a pipeline names an object here"))?;
        Ok(name.to_owned())
    }
    /// A parenthesized list of names.
    fn names(&mut self) -> Result<Vec<String>> {
        self.word("(")?;
        let mut out = vec![self.name()?];
        while self.peek() == Some(",") {
            self.next();
            out.push(self.name()?);
        }
        self.word(")")?;
        Ok(out)
    }
}

impl Pipeline {
    /// The pipeline `text` defines, `CREATE VECTOR PIPELINE` first or not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not one; SQLCODE -301 for a CHUNK clause
    /// CHUNK_TEXT refuses.
    pub fn parse(text: &str) -> Result<Pipeline> {
        let mut t = Tokens { tokens: tokens(text), at: 0 };
        if t.peek() == Some("CREATE") {
            t.next();
            t.word("VECTOR")?;
            t.word("PIPELINE")?;
        }
        let name = t.name()?;
        t.word("SOURCE")?;
        let source = t.name()?;
        let mut keys = t.names()?;
        let column = keys.pop().filter(|_| !keys.is_empty()).ok_or_else(|| refused("a pipeline's source names its keys, then its text"))?;
        let mut chunk = Spec::default();
        if t.peek() == Some("CHUNK") {
            t.next();
            let mut words = Vec::new();
            while let Some(word) = t.peek().filter(|w| *w != "EMBED") {
                words.push(word.to_owned());
                t.next();
            }
            chunk = Spec::parse(&words.join(" "))?;
        }
        t.word("EMBED")?;
        t.word("USING")?;
        let model = t.name()?;
        t.word("TARGET")?;
        let target = t.name()?;
        let columns = t.names()?;
        let mode = Self::mode(&mut t)?;
        if columns.len() != keys.len().saturating_add(3) {
            return Err(refused("a pipeline's target names its keys, then chunk id, chunk text and embedding"));
        }
        Ok(Pipeline { name, source, keys, text: column, chunk, model, target, columns, mode })
    }

    /// Its MODE clause, the end.
    fn mode(t: &mut Tokens) -> Result<Mode> {
        let mode = match t.next() {
            None => return Ok(Mode::Sync),
            Some("MODE") => match t.next() {
                Some("SYNC") => Mode::Sync,
                Some("ASYNC") => Mode::Async,
                Some(_) | None => return Err(refused("a pipeline's MODE is SYNC or ASYNC")),
            },
            Some(other) => return Err(refused(&format!("a pipeline ends before {other}"))),
        };
        if let Some(other) = t.next() {
            return Err(refused(&format!("a pipeline ends before {other}")));
        }
        Ok(mode)
    }

    /// The insert of the chunks of the rows `from` gives, `row` naming
    /// the source row in it.
    fn insert(&self, row: &str, from: &str) -> String {
        let keys: Vec<String> = self.keys.iter().map(|k| format!("{row}.{k}")).collect();
        let call = format!("CHUNK_TEXT({row}.{}, {})", self.text, quote(&self.chunk.written()));
        format!(
            "INSERT INTO {} ({}) SELECT {}, U.CHUNK_ID, U.CHUNK_TEXT, EMBED({}, U.CHUNK_TEXT) FROM {from}UNNEST({call}) AS U (CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT)",
            self.target,
            self.columns.join(", "),
            keys.join(", "),
            quote(&self.model),
        )
    }

    /// The delete of the chunks of the source row `row` names.
    fn delete(&self, row: &str) -> String {
        let matched: Vec<String> = self.keys.iter().zip(&self.columns).map(|(k, c)| format!("{c} = {row}.{k}")).collect();
        format!("DELETE FROM {} WHERE {}", self.target, matched.join(" AND "))
    }

    /// The statements that fill its target and keep it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its model is not registered.
    pub fn plan(&self, models: &Embedders) -> Result<Plan> {
        shape_of(models, &self.model)?;
        let (old, new) = ("PIPELINE_OLD", "PIPELINE_NEW");
        let watched = self.keys.iter().chain(std::iter::once(&self.text)).cloned().collect::<Vec<_>>().join(", ");
        let trigger = |suffix: &str, event: &str, referencing: &str, action: String| {
            format!("CREATE TRIGGER {}_{suffix} AFTER {event} ON {} REFERENCING {referencing} FOR EACH ROW {action}", self.name, self.source)
        };
        let statements = vec![
            format!("DELETE FROM {}", self.target),
            self.insert("S", &format!("{} AS S, ", self.source)),
            trigger("INSERT", "INSERT", &format!("NEW ROW AS {new}"), self.insert(new, "")),
            trigger("DELETE", "DELETE", &format!("OLD ROW AS {old}"), self.delete(old)),
            trigger("UPDATE_OLD", &format!("UPDATE OF {watched}"), &format!("OLD ROW AS {old}"), self.delete(old)),
            trigger("UPDATE_NEW", &format!("UPDATE OF {watched}"), &format!("NEW ROW AS {new}"), self.insert(new, "")),
        ];
        Ok(Plan { statements, hooks: hooks(self.mode) })
    }
}
