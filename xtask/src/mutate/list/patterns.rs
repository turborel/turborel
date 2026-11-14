//! The mutations of row pattern recognition (RULES.md R-TEST-9): the
//! matcher's order of preference, empty iterations, anchors, exclusion and
//! PERMUTE; the parser's quantifiers, skips and refusals; the binder's
//! navigation, semantics, skips, output columns and Syntax Rules; and the
//! executor's skips, running values, offsets, empty and unmatched rows,
//! and a window's narrowed frames.

use super::{Mutation, Package};

/// The matcher's search.
const SEARCH: &str = "src/pattern/search.rs";
/// The matcher's compiler.
const COMPILE: &str = "src/pattern/compile.rs";
/// The parser of a pattern.
const ROW_PATTERN: &str = "src/parser/pattern/row_pattern.rs";
/// The parser of the clause.
const CLAUSE: &str = "src/parser/pattern.rs";
/// The binder's navigation and set functions.
const BOUND: &str = "src/binder/pattern.rs";
/// The binder of MATCH_RECOGNIZE in FROM.
const TABLE: &str = "src/binder/pattern/table.rs";
/// The binder of a window's row pattern.
const WINDOW: &str = "src/binder/pattern/window.rs";
/// The executor's matches of a partition.
const MATCHES: &str = "src/exec/pattern/matches.rs";
/// What a definition or a measure reads.
const SCENE: &str = "src/exec/pattern/scene.rs";
/// A window's narrowed frames.
const MATCHED: &str = "src/exec/window/matched.rs";

/// The brute-force test of the search.
const BRUTE: &str = "pattern::tests::brute::the_search_agrees_with_the_language_in_preferment_order";
/// The integration test of ONE ROW PER MATCH.
const ONE_ROW: &str = "row_patterns::one_row_per_match_gives_each_matchs_measures_7_7";
/// The integration test of ALL ROWS PER MATCH.
const ALL_ROWS: &str = "row_patterns::all_rows_per_match_gives_each_rows_running_measures_7_7";
/// The integration test of empty matches and unmatched rows.
const EMPTY: &str = "row_patterns::empty_matches_and_unmatched_rows_7_7";
/// The integration test of where the next match is sought.
const SKIPS: &str = "row_patterns::where_the_next_match_is_sought_7_7";
/// The integration test of navigation.
const NAVIGATION: &str = "row_patterns::navigation_picks_a_row_and_moves_6_27";
/// The integration test of the preferred match.
const PREFERRED: &str = "row_patterns::the_preferred_match_9_41";
/// The integration test of excluded rows.
const EXCLUDED: &str = "row_patterns::excluded_rows_give_no_row_7_7";
/// The integration test of the Syntax Rules.
const REFUSED: &str = "row_patterns::the_syntax_rules_refuse_7_9";
/// The integration test of a window's narrowed frames.
const NARROWED: &str = "row_pattern_windows::a_frame_is_narrowed_to_its_match_7_15";
/// The integration test of SEEK.
const SEEK: &str = "row_pattern_windows::seek_takes_the_first_match_of_the_frame_7_15";
/// The integration test of a window's rules.
const WINDOW_RULES: &str = "row_pattern_windows::a_windows_row_pattern_rules_7_15";
/// The integration test of an ordered set function's argument not aggregated.
const ORDERED: &str = "row_pattern_rules::an_ordered_set_functions_argument_is_of_the_partition_6_7";
/// The integration test of RUNNING and FINAL outside row pattern matching.
const RUNNING: &str = "row_pattern_rules::running_or_final_in_row_pattern_matching_alone_6_9";

/// The mutations of row pattern recognition, in the order they run.
pub const PATTERNS: [Mutation; 38] = [
    Mutation {
        name: "pattern-greedy-prefers-fewer",
        package: Package::Core,
        breaks: "a greedy quantifier prefers fewer iterations and a reluctant one more [§9.41 GR6b iii, GR6c iii]",
        file: SEARCH,
        original: "let (preferred, later) = if reluctant {",
        replacement: "let (preferred, later) = if !reluctant {",
        integration: &[PREFERRED],
        unit: &["pattern::tests::greedy_prefers_more_and_reluctant_fewer", "pattern::tests::the_preferment_examples_of_note_46", BRUTE],
    },
    Mutation {
        name: "pattern-alternation-prefers-later",
        package: Package::Core,
        breaks: "an alternation prefers its later alternatives, PERMUTE its later permutations [§9.41 GR6a]",
        file: SEARCH,
        original: "                self.keep(*later);\n                Next::Go(after)",
        replacement: "                self.keep(after);\n                Next::Go(*later)",
        integration: &[PREFERRED],
        unit: &["pattern::tests::alternation_prefers_the_earlier", "pattern::tests::the_preferment_example_of_note_477", BRUTE],
    },
    Mutation {
        name: "pattern-empty-iteration-loops",
        package: Package::Core,
        breaks: "an iteration of a quantifier matching no rows does not end it [§9.41 GR6b i]",
        file: SEARCH,
        original: "Next::Go(if empty && count >= *min { *exit } else { *again })",
        replacement: "Next::Go(*again)",
        integration: &[],
        unit: &["pattern::tests::empty_iterations_end_a_quantifier", BRUTE],
    },
    Mutation {
        name: "pattern-empty-iteration-before-min",
        package: Package::Core,
        breaks: "an iteration past the least a quantifier asks is taken as empty [§9.41 GR6b i]",
        file: SEARCH,
        original: "let empty = began == self.position();",
        replacement: "let empty = began == self.position() || count > *min;",
        integration: &[],
        unit: &["pattern::tests::empty_iterations_end_a_quantifier", "pattern::tests::greedy_prefers_more_and_reluctant_fewer", BRUTE],
    },
    Mutation {
        name: "pattern-start-anchor-ignored",
        package: Package::Core,
        breaks: "^ matches anywhere, not only at a partition's start [§9.41 GR3]",
        file: SEARCH,
        original: "Instr::Start => held(self.position() == self.first),",
        replacement: "Instr::Start => held(true),",
        integration: &[PREFERRED],
        unit: &["pattern::tests::start_and_end_anchor", BRUTE],
    },
    Mutation {
        name: "pattern-excluded-flag-dropped",
        package: Package::Core,
        breaks: "rows mapped within {- -} are not marked excluded [§7.7 GR11b ii 2 B I]",
        file: COMPILE,
        original: "Pattern::Excluded(pattern) => self.pattern(pattern, within.inner(true)?),",
        replacement: "Pattern::Excluded(pattern) => self.pattern(pattern, within.inner(false)?),",
        integration: &[EXCLUDED],
        unit: &["pattern::tests::excluded_rows_are_marked", BRUTE],
    },
    Mutation {
        name: "pattern-permute-tail-unreversed",
        package: Package::Core,
        breaks: "PERMUTE's permutations are not tried in lexicographic order [§7.9 SR21]",
        file: COMPILE,
        original: "        tail.reverse();",
        replacement: "        tail.rotate_left(0);",
        integration: &[],
        unit: &["pattern::tests::permute_tries_permutations_lexicographically", BRUTE],
    },
    Mutation {
        name: "pattern-holds-error-swallowed",
        package: Package::Core,
        breaks: "an exception raised by a definition is taken as false, not raised",
        file: SEARCH,
        original: "        holds(mapped.variable, &self.path)\n",
        replacement: "        Ok(holds(mapped.variable, &self.path).unwrap_or(false))\n",
        integration: &[],
        unit: &["pattern::tests::an_error_of_holds_is_returned"],
    },
    Mutation {
        name: "pattern-star-least-one",
        package: Package::Core,
        breaks: "* is read as + [§7.9 SR19a]",
        file: ROW_PATTERN,
        original: "if self.accept_delim(delim::ASTERISK) {\n            (0, None)",
        replacement: "if self.accept_delim(delim::ASTERISK) {\n            (1, None)",
        integration: &[EMPTY],
        unit: &["parser::pattern::row_pattern::tests::each_quantifier_is_read_as_its_bounds_7_9"],
    },
    Mutation {
        name: "pattern-reluctant-lost",
        package: Package::Core,
        breaks: "a ? after a quantifier does not make it reluctant [§7.9]",
        file: ROW_PATTERN,
        original: "let reluctant = if bounds.may_be_reluctant { self.accept_delim(delim::QUESTION) } else { false };",
        replacement: "let reluctant = if bounds.may_be_reluctant { self.accept_delim(delim::QUESTION) && false } else { false };",
        integration: &[PREFERRED],
        unit: &["parser::pattern::row_pattern::tests::a_question_mark_after_a_quantifier_makes_it_reluctant_7_9"],
    },
    Mutation {
        name: "pattern-skip-default-next-row",
        package: Package::Core,
        breaks: "AFTER MATCH SKIP not written is SKIP TO NEXT ROW, not PAST LAST ROW [§7.9 SR23a]",
        file: CLAUSE,
        original: "            SkipTo::PastLastRow\n        };",
        replacement: "            SkipTo::NextRow\n        };",
        integration: &[ONE_ROW],
        unit: &["parser::pattern::tests::each_skip_to_and_its_default_7_9"],
    },
    Mutation {
        name: "pattern-initial-in-match-recognize",
        package: Package::Core,
        breaks: "MATCH_RECOGNIZE takes INITIAL or SEEK [§7.9 SR3a]",
        file: CLAUSE,
        original: "        if !window {\n            return Err(self.rule(\"[ISO/IEC 9075-2:2023 §7.9 SR3a]",
        replacement: "        if window && !window {\n            return Err(self.rule(\"[ISO/IEC 9075-2:2023 §7.9 SR3a]",
        integration: &[],
        unit: &["parser::pattern::tests::match_recognize_has_no_initial_or_seek_7_9"],
    },
    Mutation {
        name: "pattern-physical-offset-zero",
        package: Package::Core,
        breaks: "PREV and NEXT without an offset move no row, not one [§6.27 SR5c ii]",
        file: BOUND,
        original: "Some(Physical { next, offset }) => (*next, self.offset(offset.as_deref(), 1)?),",
        replacement: "Some(Physical { next, offset }) => (*next, self.offset(offset.as_deref(), 0)?),",
        integration: &[ONE_ROW, NAVIGATION],
        unit: &[],
    },
    Mutation {
        name: "pattern-negative-literal-taken",
        package: Package::Core,
        breaks: "a negative literal offset of a navigation is taken, raising as it runs [§6.27 SR5c i, SR6a]",
        file: BOUND,
        original: "{ d.units < 0 } else { false };",
        replacement: "{ d.units < 0 && false } else { false };",
        integration: &[NAVIGATION],
        unit: &[],
    },
    Mutation {
        name: "pattern-one-variable-unchecked",
        package: Package::Core,
        breaks: "a navigation or an aggregated argument reads columns of several variables [§6.27 SR4, §10.9 SR15]",
        file: BOUND,
        original: "if self.found.get().is_some_and(|v| v != variable) {",
        replacement: "if self.found.get().is_some_and(|v| v != variable) && false {",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "pattern-aggregate-final-by-default",
        package: Package::Core,
        breaks: "a set function of a measure is FINAL, not RUNNING, when neither is written [§4.21]",
        file: BOUND,
        original: "let final_ = r.semantics.take() == Some(Semantics::Final);",
        replacement: "let final_ = r.semantics.take() != Some(Semantics::Running);",
        integration: &[EMPTY],
        unit: &[],
    },
    Mutation {
        name: "pattern-order-columns-dropped",
        package: Package::Core,
        breaks: "ALL ROWS PER MATCH leaves out the ordering's columns after the partitioning's [§7.7 SR8]",
        file: TABLE,
        original: "        out.extend(keys.order.iter().map(|(k, _, _)| *k));",
        replacement: "        out.extend(keys.order.iter().map(|(k, _, _)| *k).take(0));",
        integration: &[ALL_ROWS],
        unit: &[],
    },
    Mutation {
        name: "pattern-window-columns-unread",
        package: Package::Core,
        breaks: "a column a window's row pattern alone reads is not read from its table",
        file: WINDOW,
        original: "        self.note_read(&measures, &matching)?;",
        replacement: "        if measures.values.is_empty() {\n            self.note_read(&measures, &matching)?;\n        }",
        integration: &[NARROWED, SEEK],
        unit: &[],
    },
    Mutation {
        name: "pattern-window-frame-start-unchecked",
        package: Package::Core,
        breaks: "a window's row pattern takes a frame not starting at CURRENT ROW [§7.15 SR14c]",
        file: WINDOW,
        original: "if !matches!(f.start, FrameBound::CurrentRow) || f.end.is_none() {",
        replacement: "if f.end.is_none() {",
        integration: &[WINDOW_RULES],
        unit: &[],
    },
    Mutation {
        name: "pattern-skip-past-last-as-next-row",
        package: Package::Core,
        breaks: "SKIP PAST LAST ROW seeks the next match from the row after the match's first [§7.7 GR8b ii]",
        file: MATCHES,
        original: "Skip::PastLastRow => mapped.len(),",
        replacement: "Skip::PastLastRow => 1,",
        integration: &[ONE_ROW, SKIPS],
        unit: &["exec::pattern::tests::one_row_per_match_gives_each_retained_match_7_7"],
    },
    Mutation {
        name: "pattern-running-read-as-final",
        package: Package::Core,
        breaks: "a RUNNING value reads the whole match, not the rows up to the current one [§4.21]",
        file: SCENE,
        original: "if final_ { mapped.len() } else { upto.min(mapped.len()) }",
        replacement: "if final_ { mapped.len() } else { mapped.len() }",
        integration: &[ALL_ROWS],
        unit: &["exec::pattern::tests::all_rows_per_match_reads_running_and_final_values_4_23"],
    },
    Mutation {
        name: "pattern-window-seek-ignored",
        package: Package::Core,
        breaks: "SEEK takes only a match starting at the current row, as INITIAL does [§7.15 GR6d iii 2 B II]",
        file: MATCHED,
        original: "let tried = if self.matching.matching.seek {",
        replacement: "let tried = if !self.matching.matching.seek && false {",
        integration: &[SEEK],
        unit: &["exec::window::matched::tests::seek_finds_a_later_match_initial_does_not_7_15"],
    },
    Mutation {
        name: "pattern-quantifier-zero-most-taken",
        package: Package::Core,
        breaks: "a quantifier of most 0 is taken [§7.9 SR20]",
        file: ROW_PATTERN,
        original: "&& (bounds.min > max || max == 0)",
        replacement: "&& (bounds.min > max)",
        integration: &[],
        unit: &["parser::pattern::row_pattern::tests::a_quantifiers_bounds_are_ordered_and_its_most_positive_7_9"],
    },
    Mutation {
        name: "pattern-skip-to-read-as-first",
        package: Package::Core,
        breaks: "SKIP TO v is read as SKIP TO FIRST v, not LAST [§7.9 SR23b]",
        file: CLAUSE,
        original: "let first = self.accept_kw(kw::FIRST);",
        replacement: "let first = self.accept_kw(kw::FIRST) || !self.is_kw(kw::LAST);",
        integration: &[SKIPS],
        unit: &["parser::pattern::tests::each_skip_to_and_its_default_7_9"],
    },
    Mutation {
        name: "pattern-final-navigation-read-running",
        package: Package::Core,
        breaks: "FINAL FIRST and FINAL LAST are read as RUNNING [§6.27]",
        file: "src/parser/pattern/navigation.rs",
        original: "let final_ = self.accept_word(FINAL);",
        replacement: "let final_ = self.accept_word(FINAL) && false;",
        integration: &[ALL_ROWS],
        unit: &["parser::pattern::navigation::tests::each_simple_navigation_6_27"],
    },
    Mutation {
        name: "pattern-match-recognize-taken-as-correlation",
        package: Package::Core,
        breaks: "MATCH_RECOGNIZE after a table is read as its correlation name [§7.6]",
        file: "src/parser/query.rs",
        original: " || self.at_match_recognize() {",
        replacement: " {",
        integration: &[ONE_ROW],
        unit: &["parser::pattern::tests::match_recognize_is_no_correlation_name_7_6"],
    },
    Mutation {
        name: "pattern-anchors-taken-in-a-window",
        package: Package::Core,
        breaks: "a window's pattern takes ^ and $ [§7.15 SR14e]",
        file: WINDOW,
        original: "RowPattern::Start | RowPattern::End => return true,",
        replacement: "RowPattern::Start | RowPattern::End => {}",
        integration: &[WINDOW_RULES],
        unit: &[],
    },
    Mutation {
        name: "pattern-subquery-variable-taken",
        package: Package::Core,
        breaks: "a subquery of the measures or definitions references a row pattern variable [§7.8 SR3, §7.9 SR14]",
        file: WINDOW,
        original: "if subqueries.iter().any(|s| !s.outer.is_empty()) {",
        replacement: "if subqueries.iter().any(|s| !s.outer.is_empty()) && false {",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "pattern-skip-to-last-reads-first",
        package: Package::Core,
        breaks: "SKIP TO LAST v seeks the next match from v's first row [§7.7 GR8b iii 3]",
        file: MATCHES,
        original: "Skip::Last(v) => reading.skipped_to(mapped, (v, true), SKIP_RULES)?,",
        replacement: "Skip::Last(v) => reading.skipped_to(mapped, (v, false), SKIP_RULES)?,",
        integration: &[SKIPS],
        unit: &["exec::pattern::tests::skips_seek_the_next_match_where_they_say_7_7"],
    },
    Mutation {
        name: "pattern-physical-offset-sign-flipped",
        package: Package::Core,
        breaks: "PREV moves forward and NEXT back [§6.27 GR]",
        file: SCENE,
        original: "let target = if n.next {",
        replacement: "let target = if !n.next {",
        integration: &[NAVIGATION],
        unit: &["exec::pattern::tests::navigation_moves_by_logical_then_physical_offsets_6_27"],
    },
    Mutation {
        name: "pattern-empty-match-row-omitted",
        package: Package::Core,
        breaks: "an empty match gives no row under SHOW EMPTY MATCHES [§7.7 GR11b ii 1]",
        file: MATCHES,
        original: "if len == 0 { first.then_some((self.start, 0)) }",
        replacement: "if len == 0 { None }",
        integration: &[EMPTY],
        unit: &["exec::pattern::tests::empty_matches_give_rows_of_no_rows_7_7"],
    },
    Mutation {
        name: "pattern-unmatched-ignores-matches",
        package: Package::Core,
        breaks: "WITH UNMATCHED ROWS gives rows an earlier match covered [§7.7 GR11b iii]",
        file: MATCHES,
        original: "self.covered = self.covered.max(k.saturating_add(mapped.len().max(1)));",
        replacement: "self.covered = k;",
        integration: &[EMPTY],
        unit: &["exec::pattern::tests::unmatched_rows_are_those_no_match_maps_7_7"],
    },
    Mutation {
        name: "pattern-negative-offset-accepted",
        package: Package::Core,
        breaks: "a negative offset parameter is taken, not SQLSTATE 2202J [§6.27 GR2]",
        file: SCENE,
        original: "if d.units >= 0 {",
        replacement: "if d.units >= i128::MIN {",
        integration: &[NAVIGATION],
        unit: &["exec::pattern::tests::negative_or_null_offsets_raise_6_27"],
    },
    Mutation {
        name: "pattern-excluded-rows-given",
        package: Package::Core,
        breaks: "rows mapped within {- -} give rows of ALL ROWS PER MATCH [§7.7 GR11b ii 2 B I]",
        file: MATCHES,
        original: ".take_while(|m| m.excluded)",
        replacement: ".take_while(|m| m.excluded && m.variable == usize::MAX)",
        integration: &[EXCLUDED],
        unit: &["exec::pattern::tests::excluded_rows_give_no_rows_7_7"],
    },
    Mutation {
        name: "pattern-window-skipped-rows-matched",
        package: Package::Core,
        breaks: "a row a window's match skipped past has its own match, not an empty frame [§7.15 GR6d iii 2 A]",
        file: MATCHED,
        original: "if current.row < g.skipped {",
        replacement: "if current.row < g.skipped.min(1).saturating_sub(1) {",
        integration: &[NARROWED],
        unit: &["exec::window::matched::tests::frames_are_narrowed_to_designated_matches_7_15"],
    },
    Mutation {
        name: "pattern-ordered-measure-unregistered",
        package: Package::Core,
        breaks: "an ordered set function of a measure is registered as a grouped query's, of no match [§10.9 GR1a to c]",
        file: BOUND,
        original: "|| std::ptr::eq(g, std::ptr::from_ref(&r.invariant));",
        replacement: "|| false;",
        integration: &[ORDERED],
        unit: &[],
    },
    Mutation {
        name: "pattern-invariant-partition-column-refused",
        package: Package::Core,
        breaks: "a partitioning column in an ordered set function's argument not aggregated is refused [§6.7 SR9]",
        file: BOUND,
        original: "if self.partition.contains(&column) || ",
        replacement: "if false || ",
        integration: &[ORDERED],
        unit: &[],
    },
    Mutation {
        name: "pattern-running-in-subquery-refused",
        package: Package::Core,
        breaks: "RUNNING in a subquery of a measure is refused as though outside row pattern matching [§6.9 SR11b]",
        file: BOUND,
        original: "if sf.semantics.is_some() && !self.in_pattern() {",
        replacement: "if sf.semantics.is_some() {",
        integration: &[RUNNING],
        unit: &[],
    },
];
