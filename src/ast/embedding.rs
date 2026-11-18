//! \[EXTENSION\] Abstract syntax of the statements of embedding models and
//! vector pipelines (the vector design's §12.1, §12.5).

use super::TableName;
use crate::catalog::ModelOption;

/// \[EXTENSION\] A statement that defines or alters an embedding model or
/// defines a vector pipeline.
#[derive(Debug, Clone)]
pub enum VectorStatement {
    /// CREATE EMBEDDING MODEL.
    Model(ModelDefinition),
    /// ALTER EMBEDDING MODEL.
    AlterModel(ModelDefinition),
    /// CREATE VECTOR PIPELINE.
    Pipeline(Box<PipelineDefinition>),
}

impl VectorStatement {
    /// The name of the object it defines or alters.
    pub fn name(&self) -> &TableName {
        match self {
            VectorStatement::Model(m) | VectorStatement::AlterModel(m) => &m.name,
            VectorStatement::Pipeline(p) => &p.name,
        }
    }
}

/// \[EXTENSION\] `<embedding model definition>` ::= CREATE EMBEDDING MODEL
/// `<model name>` \[FROM FILE `<character string literal>`\] WITH (
/// `<model option>` \[{, `<model option>`}...\] ); and ALTER EMBEDDING
/// MODEL `<model name>` WITH ( ... ), whose options are merged into those
/// it has.
#[derive(Debug, Clone)]
pub struct ModelDefinition {
    /// The `<model name>`.
    pub name: TableName,
    /// FROM FILE's path, if written.
    pub file: Option<String>,
    /// The options, in the order written.
    pub options: Vec<ModelOption>,
}

/// \[EXTENSION\] `<vector pipeline definition>` ::= CREATE VECTOR PIPELINE
/// `<pipeline name>` SOURCE `<table name>` ( `<column name list>` )
/// \[CHUNK `<chunk specification>`\] EMBED USING `<model name>` TARGET
/// `<table name>` ( `<column name list>` ) \[MODE { SYNC | ASYNC }\]
#[derive(Debug, Clone)]
pub struct PipelineDefinition {
    /// The `<pipeline name>`.
    pub name: TableName,
    /// The source table.
    pub source: TableName,
    /// The source's key columns.
    pub keys: Vec<String>,
    /// The source's column of text, the last named.
    pub text: String,
    /// The chunking, as CHUNK_TEXT writes its specification; none for its
    /// default.
    pub chunk: Option<String>,
    /// The model.
    pub model: TableName,
    /// The target table.
    pub target: TableName,
    /// The target's columns.
    pub columns: Vec<String>,
    /// MODE ASYNC.
    pub asynchronous: bool,
}

/// \[EXTENSION\] What DROP names of the vector design's objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorObject {
    /// An embedding model.
    Model,
    /// A vector pipeline.
    Pipeline,
}
