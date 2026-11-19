//! \[EXTENSION\] CREATE and DROP VECTOR PIPELINE (the vector design's
//! §12.5). A pipeline keeps a table of chunks in step with a table of
//! texts:
//!
//! ```text
//! CREATE VECTOR PIPELINE p SOURCE s (key..., text) [CHUNK BY ...]
//!     EMBED USING m TARGET t (key..., chunk_id, chunk_text, embedding
//!     [, version]) [MODE SYNC | ASYNC]
//! ```
//!
//! Its target exists already. As it is made, its target's rows are
//! deleted and the chunks of every source row inserted, in the creating
//! transaction. Four row triggers on the source, made with it, keep the
//! target: after an insert the new row's chunks are added, after a delete
//! the old row's removed, and after an update of the text or a key the
//! old row's removed and the new row's added. MODE SYNC embeds a chunk as
//! it is inserted, with the model's version tag if the target names a
//! version column (R12.5); MODE ASYNC leaves both null, for the queue
//! after commit (`queue.rs`). DROP VECTOR PIPELINE drops the triggers and
//! keeps the target's rows.

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::{Session, State, StatementContext};
use super::{delimited, refused, written};
use crate::ast::{PipelineDefinition, SchemaStatement, SqlStatement, VectorStatement};
use crate::binder::qualify;
use crate::catalog::{PipelineDesc, QualifiedName, TableDesc};
use crate::error::{At, Result};
use crate::params::Params;
use crate::parser::Parser;
use crate::txn::{LockMode, Txn};
use crate::vector::chunk::{Spec, quote};

/// The name a trigger's action gives the source's old row.
const OLD: &str = "PIPELINE_OLD";
/// The name it gives the new row.
const NEW: &str = "PIPELINE_NEW";

impl PipelineDesc {
    /// The target's columns a chunk's insert writes: all, or of MODE ASYNC
    /// those before its embedding.
    fn inserted_columns(&self) -> Vec<String> {
        let written = if self.asynchronous { self.keys.len().saturating_add(2) } else { self.columns.len() };
        self.columns.iter().take(written).map(|c| delimited(c)).collect()
    }

    /// The insert of the chunks of the rows `from` gives, `row` naming the
    /// source row in it.
    fn insert(&self, row: &str, from: &str) -> String {
        let mut values: Vec<String> = self.keys.iter().map(|k| format!("{row}.{}", delimited(k))).collect();
        values.push("U.CHUNK_ID".to_owned());
        values.push("U.CHUNK_TEXT".to_owned());
        let model = quote(&written(&self.model));
        if !self.asynchronous {
            values.push(format!("EMBED({model}, U.CHUNK_TEXT)"));
            if self.version_column().is_some() {
                values.push(format!("EMBED_VERSION({model}, U.CHUNK_TEXT)"));
            }
        }
        let call = format!("CHUNK_TEXT({row}.{}, {})", delimited(&self.text), quote(&self.chunk));
        format!(
            "INSERT INTO {} ({}) SELECT {} FROM {from}UNNEST({call}) AS U (CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT)",
            written(&self.target),
            self.inserted_columns().join(", "),
            values.join(", "),
        )
    }

    /// The delete of the chunks of the source row `row` names.
    fn delete(&self, row: &str) -> String {
        let matched: Vec<String> =
            self.keys.iter().zip(&self.columns).map(|(k, c)| format!("{} = {row}.{}", delimited(c), delimited(k))).collect();
        format!("DELETE FROM {} WHERE {}", written(&self.target), matched.join(" AND "))
    }

    /// The definitions of its four triggers, in the order made.
    pub(in crate::db) fn trigger_texts(&self) -> Vec<String> {
        let watched: Vec<String> = self.keys.iter().chain(std::iter::once(&self.text)).map(|c| delimited(c)).collect();
        let update = format!("UPDATE OF {}", watched.join(", "));
        let events = [
            ("INSERT", NEW, self.insert(NEW, "")),
            ("DELETE", OLD, self.delete(OLD)),
            (update.as_str(), OLD, self.delete(OLD)),
            (update.as_str(), NEW, self.insert(NEW, "")),
        ];
        let names = self.triggers();
        names
            .iter()
            .zip(events)
            .map(|(name, (event, row, action))| {
                let kind = if row == NEW { "NEW" } else { "OLD" };
                let (name, source) = (written(name), written(&self.source));
                format!("CREATE TRIGGER {name} AFTER {event} ON {source} REFERENCING {kind} ROW AS {row} FOR EACH ROW {action}")
            })
            .collect()
    }

    /// The statements that fill its target as it is made.
    fn filling(&self) -> [String; 2] {
        [format!("DELETE FROM {}", written(&self.target)), self.insert("S", &format!("{} AS S, ", written(&self.source)))]
    }
}

/// That `table` has each of `columns`, as the pipeline's `role` names them.
fn columns_of(table: &TableDesc, columns: &[String], role: &str) -> Result<()> {
    match columns.iter().find(|c| !table.columns.iter().any(|t| t.name == **c)) {
        Some(c) => Err(refused(format!("the pipeline's {role} {} has no column {c}", table.name))),
        None => Ok(()),
    }
}

impl State {
    /// CREATE VECTOR PIPELINE `p` in schema `schema`, with its triggers.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema holds a pipeline of its name, a trigger
    /// of one of its triggers' names, there is no such model, source or
    /// target, or one lacks a column it names, or the source's text is not
    /// of a character type; the triggers' own errors.
    pub(super) fn create_pipeline(&mut self, schema: &str, p: &PipelineDefinition) -> Result<Defined> {
        let qn = qualify(&p.name, schema);
        if self.catalog.embeddings().pipeline(&qn).is_some() {
            return Err(refused(format!("schema {} has a vector pipeline {} already", qn.auth, qn.ident)));
        }
        let model = qualify(&p.model, schema);
        self.model_desc(&model).or_at(p.model.at)?;
        let (source, target) = (qualify(&p.source, schema), qualify(&p.target, schema));
        let found = |t: &QualifiedName| self.catalog.table(t).cloned().ok_or_else(|| refused(format!("there is no base table {t}")));
        let (from, into) = (found(&source).or_at(p.source.at)?, found(&target).or_at(p.target.at)?);
        let read: Vec<String> = p.keys.iter().chain(std::iter::once(&p.text)).cloned().collect();
        columns_of(&from, &read, "source").or_at(p.source.at)?;
        columns_of(&into, &p.columns, "target").or_at(p.target.at)?;
        if !from.columns.iter().any(|c| c.name == p.text && c.data_type.is_character()) {
            return Err(refused(format!("the pipeline's text {} is not of a character type", p.text)).or_at(p.source.at));
        }
        let chunk = match &p.chunk {
            Some(c) => c.clone(),
            None => Spec::default().written(),
        };
        let (keys, text, columns, asynchronous) = (p.keys.clone(), p.text.clone(), p.columns.clone(), p.asynchronous);
        let desc =
            PipelineDesc { name: qn, source: source.clone(), keys, text, chunk, model, target: target.clone(), columns, asynchronous };
        if desc.asynchronous || desc.version_column().is_some() {
            self.embedders.watch();
        }
        let texts = desc.trigger_texts();
        let mut catalog = self.catalog.duplicate();
        let mut embeddings = catalog.embeddings().clone();
        embeddings.put_pipeline(desc);
        catalog.set_embeddings(embeddings);
        self.catalog = Arc::from(catalog);
        let mut defined =
            Defined { locks: vec![(source, LockMode::AccessExclusive), (target, LockMode::AccessExclusive)], ..Defined::default() };
        for trigger in texts {
            let SqlStatement::Schema { statement: SchemaStatement::Trigger(t), .. } = Parser::for_stored(&trigger)?.sql_statement()? else {
                return Err(refused(format!("{trigger} is not a trigger's definition")));
            };
            let made = self.create_trigger(schema, &t).or_at(p.name.at)?;
            defined.locks.extend(made.locks.into_iter().filter(|l| !defined.locks.contains(l)).collect::<Vec<_>>());
        }
        Ok(defined)
    }

    /// DROP VECTOR PIPELINE `qn`: its triggers, then it. Its target's rows
    /// stay.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such pipeline.
    pub(in crate::db) fn drop_pipeline(&mut self, qn: &QualifiedName) -> Result<Dropped> {
        let desc = self.catalog.embeddings().pipeline(qn).cloned().ok_or_else(|| refused(format!("there is no vector pipeline {qn}")))?;
        let mut dropped = self.drop_triggers(&desc.triggers())?;
        let mut catalog = self.catalog.duplicate();
        let mut embeddings = catalog.embeddings().clone();
        embeddings.remove_pipeline(qn);
        catalog.set_embeddings(embeddings);
        self.catalog = Arc::from(catalog);
        dropped.names.extend([qn.clone(), desc.target]);
        Ok(dropped)
    }

    /// The pipelines whose source or target is `table`, dropped with it if
    /// `cascade`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if there is one.
    pub(in crate::db) fn drop_pipelines_of(&mut self, table: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let of: Vec<QualifiedName> = self
            .catalog
            .embeddings()
            .pipelines
            .iter()
            .filter(|p| p.source == *table || p.target == *table)
            .map(|p| p.name.clone())
            .collect();
        if let (Some(first), false) = (of.first(), cascade) {
            return Err(refused(format!("vector pipeline {first} keeps {table}, so it is dropped only with CASCADE")));
        }
        let mut dropped = Dropped::default();
        for p in &of {
            dropped.absorb(self.drop_pipeline(p)?);
        }
        Ok(dropped)
    }
}

impl Session {
    /// The target of the pipeline `v` creates, filled from its source in
    /// transaction `tx`, on the definitions `next` that hold it.
    ///
    /// # Errors
    ///
    /// The statements' own.
    pub(in crate::db) fn pipeline_filled(&self, next: &Arc<State>, tx: &Txn, v: &VectorStatement) -> Result<()> {
        let VectorStatement::Pipeline(p) = v else { return Ok(()) };
        let qn = qualify(&p.name, &self.default_schema());
        let Some(desc) = next.catalog.embeddings().pipeline(&qn).cloned() else { return Ok(()) };
        let st = self.with_temporaries(Arc::clone(next));
        for text in desc.filling() {
            let statement = Parser::for_stored(&text)?.sql_statement()?;
            self.run_statement(StatementContext { st: &st, tx }, &statement, &mut Params::new(), None).or_at(p.name.at)?;
        }
        Ok(())
    }
}
