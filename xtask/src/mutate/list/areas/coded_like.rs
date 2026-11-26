//! The mutations of a LIKE judged of texts coded by symbols (RULES.md
//! R-TEST-9, D1122): the pattern's machine over codes, an escaped byte's
//! step, the late column judged of its codes, the column left late by
//! the filter, and a late column a grouping never reads dropped.

use super::super::{Mutation, Package};

/// The machine's file.
const CODES: &str = "src/value/like/codes.rs";

/// The test that compares the machine with the decoded values' match.
const AS_DECODED: &str = "value::like::codes::tests::codes_match_as_the_decoded_values";

/// The test that compares a column store's LIKE with a row store's.
const AS_ROWS: &str = "coded_like::a_like_of_coded_texts_keeps_the_rows_a_row_store_keeps";

/// Mechanisms whose removal must fail the LIKE of coded texts.
pub const CODED_LIKE: [Mutation; 5] = [
    Mutation {
        name: "coded-like-no-fallback",
        package: Package::Core,
        breaks: "a piece between that fails part way is begun again from nothing, so a value holding it overlapped is missed (RULES.md D1122)",
        file: CODES,
        original: "j = fail.get(j.saturating_sub(1)).copied().unwrap_or(0);",
        replacement: "j = 0;",
        integration: &[],
        unit: &[AS_DECODED],
    },
    Mutation {
        name: "coded-like-escape-skipped",
        package: Package::Core,
        breaks: "an escaped byte does not step the machine, so a piece holding a byte no symbol takes is missed (RULES.md D1122)",
        file: CODES,
        original: "next = self.bytes.next(at, *rest.next()?)?;",
        replacement: "next = self.bytes.next(at, 0).filter(|_| rest.next().is_some())?;",
        integration: &[],
        unit: &[AS_DECODED],
    },
    Mutation {
        name: "coded-like-not-ignored",
        package: Package::Core,
        breaks: "NOT LIKE of a late column keeps the rows LIKE keeps (RULES.md D1122)",
        file: "src/exec/batch/judge/liked.rs",
        original: "filter(|&(_, m)| m == Some(!not))",
        replacement: "filter(|&(_, m)| m == Some(true))",
        integration: &[AS_ROWS],
        unit: &[],
    },
    Mutation {
        name: "coded-like-late-dropped-read",
        package: Package::Core,
        breaks: "a grouping run in parallel drops a late column it reads, so its values are nulls (RULES.md D1122)",
        file: "src/exec/parallel/batched.rs",
        original: "if matches!(column, Column::Late(_)) && !reading.contains(&at) {",
        replacement: "if matches!(column, Column::Late(_)) {",
        integration: &[],
        unit: &["exec::parallel::batched::tests::a_late_column_not_read_is_dropped"],
    },
    Mutation {
        name: "coded-like-column-read",
        package: Package::Core,
        breaks: "a filter's LIKE of a constant reads its column, so the column is decoded, never judged of its codes (RULES.md D1122)",
        file: "src/exec/batch/late.rs",
        original: "if !matches!(like, Cond::Like(Expr::Column(_), Expr::Const(_), None | Some(Expr::Const(_)))) {",
        replacement: "if !matches!(like, Cond::Like(Expr::Column(_), Expr::Const(_), None | Some(Expr::Const(_)))) || true {",
        integration: &[],
        unit: &["exec::parallel::batched::tests::a_column_only_a_like_reads_is_given_late"],
    },
];
