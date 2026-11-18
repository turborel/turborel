//! The mutations of embedding models and vector pipelines (RULES.md
//! R-TEST-9): EXECUTE not checked, ON_ERROR's retries and nulls lost, a
//! model dropped from under what embeds by it, calls not batched, a
//! pipeline's update dropping the new row's chunks, an async pipeline's
//! insert naming its embedding, a version not embedded again, an async
//! generated column kept as its text changes, the queue's counts and the
//! hint lost, and a nested bracketed comment ending early.

use super::{Mutation, Package};

/// The mutations of embedding models and vector pipelines, in the order
/// they run.
pub const VECTOR_EMBED: [Mutation; 12] = [
    Mutation {
        name: "embed-execute-unchecked",
        package: Package::Core,
        breaks: "a call of a model is let through without EXECUTE on it",
        file: "src/binder/embedding.rs",
        original: "if !desc.execute_of(self.who(), owner_of(&*self.db.catalog, &qn.auth)).0 {",
        replacement: "if desc.execute_of(self.who(), owner_of(&*self.db.catalog, &qn.auth)).0 && false {",
        integration: &["embedding_models::execute_on_a_model_is_granted_and_revoked_r16"],
        unit: &[],
    },
    Mutation {
        name: "embed-retry-once",
        package: Package::Core,
        breaks: "ON_ERROR = 'RETRY' asks the plug-in once, never again",
        file: "src/vector/embed/model.rs",
        original: "OnError::Retry(more) => more.saturating_add(1),",
        replacement: "OnError::Retry(_) => 1,",
        integration: &["embedding_models::a_models_failures_follow_on_error_r12_4"],
        unit: &[],
    },
    Mutation {
        name: "embed-null-whole-batch",
        package: Package::Core,
        breaks: "ON_ERROR = 'NULL' nulls every text of a batch the plug-in refuses one of",
        file: "src/vector/embed/model.rs",
        original: "(OnError::Null, [_]) => Ok(vec![Value::Null]),",
        replacement: "(OnError::Null, _) => Ok(vec![Value::Null; texts.len()]),",
        integration: &["embedding_models::a_models_failures_follow_on_error_r12_4"],
        unit: &[],
    },
    Mutation {
        name: "embed-drop-model-restrict",
        package: Package::Core,
        breaks: "DROP EMBEDDING MODEL RESTRICT drops a model a generated column or pipeline embeds by",
        file: "src/db/embedding.rs",
        original: "if let (Some(user), false) = (used, cascade) {",
        replacement: "if let (Some(user), true) = (used, false) {",
        integration: &[
            "embedding_models::drop_model_restrict_and_cascade",
            "vector_pipelines::a_pipeline_is_refused_of_what_it_cannot_keep",
        ],
        unit: &[],
    },
    Mutation {
        name: "embed-calls-not-batched",
        package: Package::Core,
        breaks: "a projection asks a model for one row's text at a time",
        file: "src/exec/batch/project.rs",
        original: "    if !function.0.batches() {\n        return None;",
        replacement: "    if function.0.batches() || true {\n        return None;",
        integration: &["embedding_models::calls_are_batched_across_rows_r12_3"],
        unit: &[],
    },
    Mutation {
        name: "pipeline-update-drops-new-chunks",
        package: Package::Core,
        breaks: "a pipeline's update of a source text removes the old chunks and adds none",
        file: "src/db/embedding/pipeline.rs",
        original: "(update.as_str(), NEW, self.insert(NEW, \"\")),",
        replacement: "(update.as_str(), NEW, self.delete(NEW)),",
        integration: &["vector_pipelines::a_pipeline_keeps_its_target_in_step_r12_5"],
        unit: &[],
    },
    Mutation {
        name: "pipeline-async-inserts-embedding",
        package: Package::Core,
        breaks: "a pipeline of MODE ASYNC names its embedding column in an insert that gives it no value",
        file: "src/db/embedding/pipeline.rs",
        original: "let written = if self.asynchronous { self.keys.len().saturating_add(2) } else { self.columns.len() };",
        replacement: "let written = self.columns.len();",
        integration: &[
            "vector_pipelines::an_async_pipeline_is_filled_after_commit_r12_6",
            "vector_pipelines::the_queue_survives_a_crash_r12_6",
        ],
        unit: &[],
    },
    Mutation {
        name: "queue-version-not-reembedded",
        package: Package::Core,
        breaks: "the chunks of a pipeline of MODE SYNC are not embedded again for a model's new version",
        file: "src/db/embedding/queue.rs",
        original: "if !p.asynchronous && p.version_column().is_none() {",
        replacement: "if !p.asynchronous {",
        integration: &["vector_pipelines::a_new_version_is_embedded_again_r12_5"],
        unit: &[],
    },
    Mutation {
        name: "async-generation-kept-on-change",
        package: Package::Core,
        breaks: "an async generated column keeps its vector as an update changes the text it embeds",
        file: "src/binder/generated.rs",
        original: "*e = if changed { Expr::Const(Value::Null) } else { Expr::Column(*at) };",
        replacement: "*e = Expr::Column(*at);",
        integration: &["vector_pipelines::an_async_generated_column_is_filled_after_commit_r12_6"],
        unit: &[],
    },
    Mutation {
        name: "queue-embedded-not-counted",
        package: Package::Core,
        breaks: "EMBEDDING_QUEUE shows no vectors embedded",
        file: "src/db/embedding/queue.rs",
        original: "state.embedded = state.embedded.saturating_add(state.pending);",
        replacement: "state.embedded = state.embedded.saturating_add(0);",
        integration: &["vector_pipelines::an_async_pipeline_is_filled_after_commit_r12_6"],
        unit: &[],
    },
    Mutation {
        name: "queue-hint-ignored",
        package: Package::Core,
        breaks: "a statement hinted VECTOR_WAIT_FRESH reads what the queue has not filled yet",
        file: "src/db/embedding/queue.rs",
        original: "if self.transaction().is_some() || !crate::lexer::hinted(sql, WAIT_FRESH) {",
        replacement: "if self.transaction().is_some() || !crate::lexer::hinted(sql, WAIT_FRESH) || true {",
        integration: &["vector_pipelines::an_async_pipeline_is_filled_after_commit_r12_6"],
        unit: &[],
    },
    Mutation {
        name: "bracketed-comment-not-nested",
        package: Package::Core,
        breaks: "a bracketed comment ends at the first */, a nested one's",
        file: "src/lexer/comments.rs",
        original: "(Some('/'), Some('*')) => depth = depth.saturating_add(1),",
        replacement: "(Some('/'), Some('*')) => depth = 1,",
        integration: &["vector_pipelines::bracketed_comments_are_separators_t351"],
        unit: &[],
    },
];
