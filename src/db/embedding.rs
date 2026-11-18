//! \[EXTENSION\] Embedding models and vector pipelines as objects of a
//! schema (the vector design's §12.1, §12.5): CREATE, ALTER and DROP
//! EMBEDDING MODEL, CREATE and DROP VECTOR PIPELINE, and EXECUTE on a model
//! granted and revoked (`grants.rs`).
//!
//! A model is bound to the plug-in its TYPE names, which the host
//! registers (`Database::register_embedder`). CREATE and ALTER refuse a
//! plug-in not registered, as the session runs them. Made again by
//! recovery, a model of a plug-in the host has not registered yet keeps
//! the shape its descriptor had, so the catalog opens before the host
//! registers its plug-ins. A call names a model as EMBED('U.M', text) and
//! is bound to it (`binder/embedding.rs`).
//!
//! A pipeline is kept by four row triggers on its source, made with it
//! and dropped with it (`pipeline.rs`). Its MODE ASYNC leaves a chunk's
//! embedding null for the queue after commit (`queue.rs`).

mod grants;
mod pipeline;
mod queue;

use std::fmt::Write as _;
use std::sync::Arc;

pub(super) use queue::{EMBEDDING, embedded, in_round};

use super::ddl::Dropped;
use super::definitions::Defined;
use super::revoke::Abandoned;
use super::{Database, Session, State, rule};
use crate::ast::{Literal, ModelDefinition, SchemaStatement, SqlStatement, TableName, ValueSpecification, VectorObject, VectorStatement};
use crate::binder::{qualify, view_degree};
use crate::catalog::{ModelDesc, ModelOption, QualifiedName};
use crate::diagnostics::Extension;
use crate::error::{At, Code, Result, SqlError};
use crate::lexer::{Tok, delim, lex};
use crate::log::record::DropTarget;
use crate::params::Params;
use crate::value::{Binary, Value};
use crate::vector::chunk::quote;
use crate::vector::embed::{DIMENSIONS, Embedder, FORMAT, LAYOUT, ModelShape, SHAPE, VERSION, number_option};
use crate::vector::{Element, Layout};

/// The option naming a model's plug-in.
pub(crate) const TYPE: &str = "TYPE";

/// What DROP EMBEDDING MODEL or DROP VECTOR PIPELINE drops of `kind`,
/// named `qn`.
pub(super) fn drop_target(kind: VectorObject, qn: QualifiedName) -> DropTarget {
    match kind {
        VectorObject::Model => DropTarget::Model(qn),
        VectorObject::Pipeline => DropTarget::Pipeline(qn),
    }
}

/// The model a string names, as EMBED's first argument does: an
/// identifier, or two with a period between, each regular or delimited.
pub(crate) fn model_named(text: &str) -> Option<TableName> {
    let tokens = lex(text).ok()?;
    let words: Vec<&Tok> = tokens.iter().map(|t| &t.tok).filter(|t| !matches!(t, Tok::End)).collect();
    let ident = |t: &Tok| if let Tok::Identifier(s) = t { Some(s.clone()) } else { None };
    match words.as_slice() {
        [one] => Some(TableName { auth: None, ident: ident(one)?, at: 0 }),
        [auth, Tok::Delimiter(d), one] => {
            (*d == delim::PERIOD).then(|| Some(TableName { auth: Some(ident(auth)?), ident: ident(one)?, at: 0 }))?
        }
        _ => None,
    }
}

/// `text` as a delimited identifier.
pub(crate) fn delimited(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// `qn` as written in a statement: two delimited identifiers.
pub(crate) fn written(qn: &QualifiedName) -> String {
    format!("{}.{}", delimited(&qn.auth), delimited(&qn.ident))
}

fn refused(detail: impl std::fmt::Display) -> SqlError {
    rule(format!("[EXTENSION] {detail}"))
}

/// FROM BLOB's octets of `d`, if written: a binary string literal, as
/// `blob_given` leaves a parameter.
///
/// # Errors
///
/// SQLCODE -103 if it is a parameter given no value, or not a binary
/// string.
fn blob_of(d: &ModelDefinition) -> Result<Option<Vec<u8>>> {
    match &d.blob {
        None => Ok(None),
        Some(ValueSpecification::Literal(Literal::Binary(b))) => Ok(Some(b.bytes().to_vec())),
        Some(_) => Err(refused("FROM BLOB is a binary string literal, or a parameter given a binary string")),
    }
}

/// `stmt`, CREATE EMBEDDING MODEL FROM BLOB of a parameter, with the
/// parameter's value of `params` written in its stead, so the text logged
/// holds the octets; else as it is.
///
/// # Errors
///
/// SQLCODE -103 if the parameter's value is not a binary string, or a
/// large object's held whole.
pub(super) fn blob_given(stmt: Arc<SqlStatement>, params: &Params) -> Result<Arc<SqlStatement>> {
    let SqlStatement::Schema { statement: SchemaStatement::Vector(v), text } = &*stmt else { return Ok(stmt) };
    let VectorStatement::Model(d) = &**v else { return Ok(stmt) };
    let Some(ValueSpecification::Parameter(p)) = &d.blob else { return Ok(stmt) };
    let value = params.value(&p.name);
    let octets = if let Value::Binary(b) = value {
        b.bytes().to_vec()
    } else if let Value::Lob(lob) = value {
        lob.inline().map(<[u8]>::to_vec).ok_or_else(|| refused("FROM BLOB's large object is not held whole"))?
    } else {
        return Err(refused(format!("FROM BLOB's parameter {} is given no binary string", p.name)));
    };
    let mut given = d.clone();
    given.blob = Some(ValueSpecification::Literal(Literal::Binary(Binary::new(octets))));
    let statement = SchemaStatement::Vector(Box::new(VectorStatement::Model(given)));
    Ok(Arc::new(SqlStatement::Schema { statement, text: text.clone() }))
}

/// The plug-in `options` name by TYPE.
fn plugin_of(options: &[ModelOption]) -> Result<String> {
    let found = options.iter().find(|o| o.name == TYPE).ok_or_else(|| refused("a model's options name its plug-in as TYPE".to_owned()))?;
    Ok(found.value.trim().to_uppercase())
}

/// The shape SHAPE of `options` gives, as `shape_option` writes it; else
/// DIMENSIONS, FORMAT and LAYOUT, if all three are given.
fn written_shape(options: &[ModelOption]) -> Result<Option<ModelShape>> {
    let find = |name: &str| options.iter().find(|o| o.name == name).map(|o| o.value.clone());
    let parts: Vec<String> = match find(SHAPE) {
        Some(shape) => shape.split_whitespace().map(str::to_owned).collect(),
        None => [find(DIMENSIONS), find(FORMAT), find(LAYOUT)].into_iter().flatten().collect(),
    };
    let [dims, format, layout] = parts.as_slice() else { return Ok(None) };
    let element = Element::named(&format.to_ascii_uppercase()).ok_or_else(|| refused(format!("no element format {format}")))?;
    let layout = Layout::named(&layout.to_ascii_uppercase()).ok_or_else(|| refused(format!("no layout {layout}")))?;
    let dims = dims.parse::<u32>().map_err(|_| refused(format!("a model's dimensions are a whole number, not {dims}")))?;
    Ok(Some(ModelShape { dims, element, layout }))
}

/// The option SHAPE of `shape`.
fn shape_option(shape: ModelShape) -> ModelOption {
    let value = format!("{} {} {}", shape.dims, shape.element.name(), shape.layout.name());
    ModelOption { name: SHAPE.to_owned(), value, quoted: true }
}

/// `o` as written in a `<model option>`.
fn option_text(o: &ModelOption) -> String {
    let value = if o.quoted { quote(&o.value) } else { o.value.clone() };
    format!("{} = {value}", delimited(&o.name))
}

impl State {
    /// Makes `v`, its names in schema `schema`.
    pub(super) fn vector_statement(&mut self, schema: &str, v: &VectorStatement) -> Result<Defined> {
        match v {
            VectorStatement::Model(d) => self.create_model(schema, d).or_at(d.name.at),
            VectorStatement::AlterModel(d) => self.alter_model(schema, d).or_at(d.name.at),
            VectorStatement::Pipeline(p) => self.create_pipeline(schema, p).or_at(p.name.at),
        }
    }

    /// The model `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none.
    pub(crate) fn model_desc(&self, qn: &QualifiedName) -> Result<ModelDesc> {
        self.catalog.embeddings().model(qn).cloned().ok_or_else(|| refused(format!("there is no embedding model {qn}")))
    }

    /// The shape of a model of plug-in `plugin`, FROM FILE's `file` or
    /// FROM BLOB's octets and `options`, configured; of a plug-in not registered, the shape
    /// `options` write, else `kept`.
    fn model_shape(
        &self,
        (plugin, from): (&str, (Option<&str>, Option<&[u8]>)),
        options: &[ModelOption],
        kept: Option<ModelShape>,
    ) -> Result<ModelShape> {
        if self.embedders.named(plugin)?.is_some() {
            let given: Vec<ModelOption> = options.iter().filter(|o| o.name != TYPE).cloned().collect();
            let shape = self.embedders.configure(plugin, from, &given)?.0.shape();
            if options.iter().any(|o| o.name == SHAPE) && written_shape(options)? != Some(shape) {
                return Err(refused(format!("VEC-0031 the plug-in {plugin} no longer gives vectors of the shape its model was made of")));
            }
            return Ok(shape);
        }
        written_shape(options)?.or(kept).ok_or_else(|| refused(format!("VEC-0030 no model plug-in {plugin} is registered")))
    }

    /// CREATE EMBEDDING MODEL `d` in schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema does not exist, or holds a model of its
    /// name; or its options are refused.
    fn create_model(&mut self, schema: &str, d: &ModelDefinition) -> Result<Defined> {
        let qn = qualify(&d.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            return Err(refused(format!("there is no schema {}", qn.auth)));
        }
        if self.catalog.embeddings().model(&qn).is_some() {
            return Err(refused(format!("schema {} has an embedding model {} already", qn.auth, qn.ident)));
        }
        let plugin = plugin_of(&d.options)?;
        let blob = blob_of(d)?;
        let shape = self.model_shape((&plugin, (d.file.as_deref(), blob.as_deref())), &d.options, None)?;
        let options: Vec<ModelOption> = d.options.iter().filter(|o| o.name != TYPE && o.name != SHAPE).cloned().collect();
        let version = options.iter().find(|o| o.name == VERSION).map(number_option).transpose()?;
        let (dims, element, layout) = (shape.dims, shape.element, shape.layout);
        let file = d.file.clone();
        let desc = ModelDesc { name: qn, plugin, file, blob, options, version, dims, element, layout, execute: Vec::new() };
        self.put_model(desc);
        Ok(Defined::default())
    }

    /// ALTER EMBEDDING MODEL `d`: its options merged into the model's, a
    /// new TYPE its plug-in. A model whose version changes has its
    /// columns' vectors made again (`queue.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such model, its options are refused, or
    /// its vectors would change shape (VEC-0031).
    fn alter_model(&mut self, schema: &str, d: &ModelDefinition) -> Result<Defined> {
        let mut desc = self.model_desc(&qualify(&d.name, schema))?;
        let mut merged = desc.options.clone();
        merged.push(ModelOption { name: TYPE.to_owned(), value: desc.plugin.clone(), quoted: false });
        for o in &d.options {
            merged.retain(|m| m.name != o.name);
            merged.push(o.clone());
        }
        let kept = ModelShape { dims: desc.dims, element: desc.element, layout: desc.layout };
        desc.plugin = plugin_of(&merged)?;
        if self.model_shape((&desc.plugin, (desc.file.as_deref(), desc.blob.as_deref())), &merged, Some(kept))? != kept {
            return Err(refused(format!("VEC-0031 ALTER EMBEDDING MODEL keeps the shape of {}'s vectors", desc.name)));
        }
        merged.retain(|o| o.name != TYPE && o.name != SHAPE);
        desc.version = merged.iter().find(|o| o.name == VERSION).map(number_option).transpose()?;
        desc.options = merged;
        self.embedders.watch();
        self.put_model(desc);
        Ok(Defined::default())
    }

    /// The catalog with `desc` in place of the model of its name.
    pub(super) fn put_model(&mut self, desc: ModelDesc) {
        let mut catalog = self.catalog.duplicate();
        let mut embeddings = catalog.embeddings().clone();
        embeddings.put_model(desc);
        catalog.set_embeddings(embeddings);
        self.catalog = Arc::from(catalog);
    }

    /// DROP EMBEDDING MODEL `qn`, with the pipelines that embed by it and
    /// the views that call it, and what reads those views, if `cascade`.
    /// Its generated columns went first (`generations_dropped`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such model, or, not `cascade`, a
    /// pipeline, a generated column or a view embeds by it.
    pub(super) fn drop_model(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        self.model_desc(qn)?;
        let pipelines: Vec<QualifiedName> =
            self.catalog.embeddings().pipelines.iter().filter(|p| p.model == *qn).map(|p| p.name.clone()).collect();
        let columns = self.generations_without(&DropTarget::Model(qn.clone()));
        let views = self.views_embedding(qn);
        let used = pipelines
            .first()
            .map(|p| format!("pipeline {p}"))
            .or_else(|| columns.first().map(|(t, c)| format!("generated column {t}.{c}")))
            .or_else(|| views.first().map(|v| format!("view {v}")));
        if let (Some(user), false) = (used, cascade) {
            return Err(refused(format!("{user} embeds by model {qn}, so it is dropped only with CASCADE")));
        }
        let mut dropped = Dropped::default();
        for p in &pipelines {
            dropped.absorb(self.drop_pipeline(p)?);
        }
        dropped.absorb(self.drop_views_embedding(&views)?);
        self.remove_model(qn);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// The model `qn` gone from the catalog.
    fn remove_model(&mut self, qn: &QualifiedName) {
        let mut catalog = self.catalog.duplicate();
        let mut embeddings = catalog.embeddings().clone();
        embeddings.remove_model(qn);
        catalog.set_embeddings(embeddings);
        self.catalog = Arc::from(catalog);
    }

    /// The views that call model `qn`: those that bind no more without it,
    /// and so those that read them.
    fn views_embedding(&self, qn: &QualifiedName) -> Vec<QualifiedName> {
        let mut after = self.clone();
        after.remove_model(qn);
        self.catalog.views().into_iter().filter(|v| view_degree(&after, v).is_err()).map(|v| v.name.clone()).collect()
    }

    /// The views `views` dropped, with their privileges, and the triggers
    /// and routines that read them, as DROP ROUTINE CASCADE drops those of
    /// a routine.
    ///
    /// # Errors
    ///
    /// As each drop's.
    fn drop_views_embedding(&mut self, views: &[QualifiedName]) -> Result<Dropped> {
        if views.is_empty() {
            return Ok(Dropped::default());
        }
        let abandoned = Abandoned { views: views.to_vec(), ..Abandoned::default() };
        let (mut dropped, locks) = self.destroy(Arc::clone(&self.catalog), &abandoned)?;
        let read: Vec<&QualifiedName> = views.iter().collect();
        dropped.absorb(self.drop_triggering(&read, (true, false))?);
        dropped.absorb(self.drop_routining(&read, (true, false))?);
        dropped.names.extend(locks.into_iter().map(|(name, _)| name));
        Ok(dropped)
    }

    /// The models and pipelines of schema `auth`, pipelines first.
    pub(super) fn schema_embeddings(&self, auth: &str) -> Vec<QualifiedName> {
        let all = self.catalog.embeddings();
        let pipelines = all.pipelines.iter().map(|p| &p.name);
        pipelines.chain(all.models.iter().map(|m| &m.name)).filter(|n| n.auth == auth).cloned().collect()
    }

    /// The pipelines of schema `auth` dropped, then its models with
    /// CASCADE, as DROP SCHEMA drops them.
    ///
    /// # Errors
    ///
    /// As each drop's.
    pub(super) fn drop_schema_embeddings(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for name in self.schema_embeddings(auth) {
            if self.catalog.embeddings().pipeline(&name).is_some() {
                dropped.absorb(self.drop_pipeline(&name)?);
            } else if self.catalog.embeddings().model(&name).is_some() {
                dropped.absorb(self.drop_model(&name, true)?);
            }
        }
        Ok(dropped)
    }
}

impl Session {
    /// The text logged of CREATE EMBEDDING MODEL `statement`, in schema
    /// `schema`, if it writes no SHAPE: its name
    /// qualified, and the shape its plug-in gives as SHAPE after its
    /// options (`State::model_shape`).
    ///
    /// # Errors
    ///
    /// As the plug-in's configuration.
    pub(super) fn shaped_model(st: &State, statement: &SchemaStatement, schema: &str) -> Result<Option<String>> {
        let SchemaStatement::Vector(v) = statement else { return Ok(None) };
        let VectorStatement::Model(d) = &**v else { return Ok(None) };
        if d.options.iter().any(|o| o.name == SHAPE) {
            return Ok(None);
        }
        let blob = blob_of(d).or_at(d.name.at)?;
        let shape = st.model_shape((&plugin_of(&d.options)?, (d.file.as_deref(), blob.as_deref())), &d.options, None).or_at(d.name.at)?;
        let mut options: Vec<String> = d.options.iter().map(option_text).collect();
        options.push(option_text(&shape_option(shape)));
        let file = match (&d.file, &blob) {
            (Some(f), _) => format!(" FROM FILE {}", quote(f)),
            (None, Some(b)) => {
                let hex = b.iter().fold(String::new(), |mut hex, o| {
                    let _ = write!(hex, "{o:02X}");
                    hex
                });
                format!(" FROM BLOB X'{hex}'")
            }
            (None, None) => String::new(),
        };
        Ok(Some(format!("CREATE EMBEDDING MODEL {}{file} WITH ({})", written(&qualify(&d.name, schema)), options.join(", "))))
    }

    /// That the plug-in a CREATE or ALTER EMBEDDING MODEL `statement` names
    /// is registered, as recovery does not ask (`State::model_shape`).
    ///
    /// # Errors
    ///
    /// VEC-0030, SQLCODE -103, if it is not.
    pub(super) fn plugin_registered(st: &State, statement: &SchemaStatement, schema: &str) -> Result<()> {
        let SchemaStatement::Vector(v) = statement else { return Ok(()) };
        let (d, plugin) = match &**v {
            VectorStatement::Model(d) => (d, plugin_of(&d.options)?),
            VectorStatement::AlterModel(d) => match d.options.iter().find(|o| o.name == TYPE) {
                Some(o) => (d, o.value.trim().to_uppercase()),
                None => (d, st.model_desc(&qualify(&d.name, schema)).or_at(d.name.at)?.plugin),
            },
            VectorStatement::Pipeline(_) => return Ok(()),
        };
        if st.embedders.named(&plugin)?.is_none() {
            return Err(
                SqlError::new(Code::SyntaxRule, format!("[EXTENSION] VEC-0030 no model plug-in {plugin} is registered")).or_at(d.name.at)
            );
        }
        Ok(())
    }
}

impl Database {
    /// \[EXTENSION\] Registers `embedder`, a model plug-in, which a model of
    /// the catalog names by its option TYPE, and EMBED and CHUNK_TEXT by
    /// its name (R12.2). Its contract is `Embedder`'s. A model of the
    /// catalog bound to it before is bound again as a statement next names
    /// it, and the queue after commit runs a round, so vectors waiting for
    /// it are given.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if its name is not upper case, or one of its name is
    /// registered; -910 if the database is closed.
    pub fn register_embedder(&self, embedder: Arc<dyn Embedder>) -> Result<()> {
        let name = embedder.name().to_owned();
        let done = self.snapshot().and_then(|st| st.embedders.register(embedder));
        if done.is_ok() {
            self.request_embedding();
        }
        self.registered((Extension::Embedder, &name), done)
    }
}
