//! \[EXTENSION\] The queue of embeddings after commit (the vector design's
//! R12.6), and the vectors of a model's older version made again (R12.5).
//!
//! What waits is in the rows themselves, so it lasts as they do, through
//! a crash: a chunk of a pipeline of MODE ASYNC whose embedding is null, a
//! chunk whose version column names a version of its model other than
//! the model's, and a row of a generated column of ASYNC that is null
//! though the columns it reads are not. A round of the queue, run by the
//! job `turborel-embedding` after a commit that wrote and as the database
//! opens, gives each its vector by one UPDATE a column, in a transaction
//! of the owner of the table's schema: the chunk's EMBED and version tag,
//! or the generated column's DEFAULT. Before it, the texts waiting are
//! embedded ahead, the model asked for BATCH_SIZE of them at a time
//! (R12.3), up to `MOST_PRIMED`, so the UPDATE asks for none of them.
//!
//! A round failing, as the plug-in or the UPDATE does, leaves the rows
//! waiting for the next round, and INFORMATION_SCHEMA.EMBEDDING_QUEUE
//! shows its error. `Database::wait_for_embeddings`, and a statement
//! whose text begins with the hint `/*+ VECTOR_WAIT_FRESH */`, run a round
//! first, so what is read is fresh.

use std::sync::Arc;
use std::time::Duration;

use super::super::{Database, Session, State};
use super::{delimited, model_named, written};
use crate::ast::{Literal, ValueExpression, ValueSpecification};
use crate::binder::qualify;
use crate::catalog::{Generation, PipelineDesc, QualifiedName, owner_of};
use crate::error::Result;
use crate::parser::Parser;
use crate::value::Value;
use crate::vector::chunk::quote;
use crate::vector::embed::{BoundModel, EMBED_NAMES, MOST_PRIMED, QueueState, text_of};

/// The hint that asks for a round first.
const WAIT_FRESH: &str = "VECTOR_WAIT_FRESH";
/// The label of a database's job and thread of the queue.
pub(in crate::db) const EMBEDDING: &str = "turborel-embedding";

/// One column the queue fills: its table, the column, the model it
/// embeds by, if known, its texts' column, and the condition and SET
/// clause of its UPDATE.
struct Work {
    table: QualifiedName,
    column: String,
    model: Option<Arc<BoundModel>>,
    text: Option<String>,
    waiting: String,
    set: String,
}

/// The job's work: a round of the queue.
///
/// # Errors
///
/// The first column's error; the others are shown by EMBEDDING_QUEUE.
pub(in crate::db) fn embedded(db: &Database) -> Result<Option<Duration>> {
    let st = db.snapshot()?;
    let _turn = st.embedders.round()?;
    let mut first = None;
    for work in st.queue_work() {
        let model = work.model.as_ref().map_or_else(String::new, |m| m.tag());
        let (schema, table, column) = (work.table.auth.clone(), work.table.ident.clone(), work.column.clone());
        let mut state = st.embedders.queue()?.into_iter().find(|s| (&s.schema, &s.table, &s.column) == (&schema, &table, &column));
        let state = state.get_or_insert_with(|| QueueState { schema, table, column, ..QueueState::default() });
        state.model = model;
        match round(db, &st, &work, &mut state.pending) {
            Ok(()) => {
                state.embedded = state.embedded.saturating_add(state.pending);
                state.error = None;
            }
            Err(e) => {
                state.failed = state.failed.saturating_add(1);
                state.error = Some(e.msg.clone());
                first.get_or_insert(e);
            }
        }
        st.embedders.record(state.clone())?;
    }
    first.map_or(Ok(None), Err)
}

/// The round of `work` run on the database `db`, whose definitions are
/// `st`, the rows waiting as it began counted in `pending`.
fn round(db: &Database, st: &State, work: &Work, pending: &mut u64) -> Result<()> {
    let session = db.session(owner_of(&*st.catalog, &work.table.auth));
    let ran = filled(&session, work, pending);
    if let Some(model) = &work.model {
        model.unprime();
    }
    match ran {
        Ok(()) => session.commit(),
        Err(e) => {
            session.rollback()?;
            Err(e)
        }
    }
}

/// `round`'s statements, in the transaction of `session`.
fn filled(session: &Session, work: &Work, pending: &mut u64) -> Result<()> {
    let table = written(&work.table);
    let mut counted = session.open_cursor(&format!("SELECT COUNT(*) FROM {table} WHERE {}", work.waiting))?;
    // COUNT(*) is exact, of scale 0.
    let count = counted.fetch()?.and_then(|r| r.into_iter().next());
    *pending = if let Some(Value::Exact(n)) = count { u64::try_from(n.units).unwrap_or(0) } else { 0 };
    drop(counted);
    if *pending == 0 {
        return Ok(());
    }
    if let (Some(model), Some(text)) = (&work.model, &work.text) {
        let mut texts = Vec::new();
        let sql = format!("SELECT DISTINCT {text} FROM {table} WHERE {} FETCH FIRST {MOST_PRIMED} ROWS ONLY", work.waiting);
        let mut waiting = session.open_cursor(&sql)?;
        while let Some(row) = waiting.fetch()? {
            texts.extend(row.first().map(text_of).transpose()?.flatten());
        }
        drop(waiting);
        model.prime(&texts)?;
    }
    session.run(&format!("UPDATE {table} SET {} WHERE {}", work.set, work.waiting)).map(drop)
}

impl State {
    /// Whether the queue fills a column.
    pub(in crate::db) fn queue_due(&self) -> bool {
        !self.queue_work().is_empty()
    }

    /// What the queue fills: each pipeline of MODE ASYNC or of a version
    /// column, and each generated column of ASYNC.
    fn queue_work(&self) -> Vec<Work> {
        let mut all: Vec<Work> = self.catalog.embeddings().pipelines.iter().filter_map(|p| self.pipeline_work(p)).collect();
        for t in self.catalog.tables() {
            for c in &t.columns {
                if let Some(g) = c.generation().filter(|g| g.asynchronous) {
                    all.push(self.column_work(&t.name, (&c.name, g), t));
                }
            }
        }
        all
    }

    /// The work of pipeline `p`, if the queue fills it.
    fn pipeline_work(&self, p: &PipelineDesc) -> Option<Work> {
        let (text, embedding) = (p.chunk_text_column()?, p.embedding_column()?);
        if !p.asynchronous && p.version_column().is_none() {
            return None;
        }
        let model = self.model_desc(&p.model).and_then(|d| self.embedders.bind(&d)).ok();
        let call = format!("EMBED({}, {})", quote(&written(&p.model)), delimited(text));
        let mut waiting = format!("{} IS NULL", delimited(embedding));
        let mut set = format!("{} = {call}", delimited(embedding));
        if let (Some(version), Some(m)) = (p.version_column(), &model) {
            let (version, tag) = (delimited(version), quote(&m.tag()));
            waiting = if p.asynchronous {
                format!("({waiting} OR {version} IS NULL OR {version} <> {tag})")
            } else {
                format!("({version} IS NULL OR {version} <> {tag})")
            };
            set = format!("{set}, {version} = {tag}");
        }
        let waiting = format!("{waiting} AND {} IS NOT NULL", delimited(text));
        Some(Work { table: p.target.clone(), column: embedding.to_owned(), model, text: Some(delimited(text)), waiting, set })
    }

    /// The work of the generated column `column` of ASYNC, of generation
    /// `g`, of `table`: its rows null whose columns it reads are not.
    fn column_work(&self, table: &QualifiedName, (column, g): (&str, &Generation), desc: &crate::catalog::TableDesc) -> Work {
        let mut waiting = vec![format!("{} IS NULL", delimited(column))];
        let at = desc.columns.iter().position(|c| c.name == column).unwrap_or(0);
        if let Ok((e, _)) = crate::binder::bind_generation(self, desc, (at, g)) {
            let mut used = Vec::new();
            crate::logical::expr_columns(&e, &mut used);
            used.sort_unstable();
            used.dedup();
            waiting.extend(used.iter().filter_map(|&u| desc.columns.get(u)).map(|c| format!("{} IS NOT NULL", delimited(&c.name))));
        }
        let (model, text) = self.embedded_by(g).map_or((None, None), |(m, t)| (Some(m), Some(t)));
        Work {
            table: table.clone(),
            column: column.to_owned(),
            model,
            text,
            waiting: waiting.join(" AND "),
            set: format!("{} = DEFAULT", delimited(column)),
        }
    }

    /// The model of the catalog and the column of texts of generation `g`,
    /// if it is EMBED of a model named by a literal and of a column.
    fn embedded_by(&self, g: &Generation) -> Option<(Arc<BoundModel>, String)> {
        let ValueExpression::Call { schema: None, name, args, .. } = Parser::for_stored(&g.text).ok()?.generation_expression().ok()? else {
            return None;
        };
        let [ValueExpression::Value(ValueSpecification::Literal(Literal::Character(m))), ValueExpression::Column(c)] = args.as_slice()
        else {
            return None;
        };
        if !EMBED_NAMES.contains(&name.as_str()) || c.qualifier.is_some() {
            return None;
        }
        let desc = self.model_desc(&qualify(&model_named(m)?, &g.names)).ok()?;
        Some((self.embedders.bind(&desc).ok()?, delimited(&c.column)))
    }
}

impl Session {
    /// A round of the queue run first, if `sql` begins with the hint
    /// `/*+ VECTOR_WAIT_FRESH */` and the session has no transaction open,
    /// whose locks the round would wait for.
    ///
    /// # Errors
    ///
    /// As `Database::wait_for_embeddings`.
    pub(in crate::db) fn wait_if_hinted(&self, sql: &str) -> Result<()> {
        if self.transaction().is_some() || !crate::lexer::hinted(sql, WAIT_FRESH) {
            return Ok(());
        }
        self.db.wait_for_embeddings()
    }
}

impl Database {
    /// \[EXTENSION\] Runs a round of the queue of embeddings after commit
    /// now (R12.6): each vector waiting given, so what is read after is
    /// fresh, as `/*+ VECTOR_WAIT_FRESH */` asks.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed; the first column's error.
    pub fn wait_for_embeddings(&self) -> Result<()> {
        embedded(self).map(drop)
    }
}
