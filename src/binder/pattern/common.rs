//! [ISO/IEC 9075-2:2023 §7.9] A `<row pattern common syntax>` bound: its
//! row pattern variables, its `<row pattern>`, where the next match is
//! sought after one, and its definitions.

use super::Recognizing;
use crate::ast::{PatternCommon, RowPattern, SkipTo};
use crate::binder::Binder;
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Cond, Matching, Skip, Slotted, Variable};
use crate::parser::MAX_NESTING_DEPTH;
use crate::pattern::Pattern;

/// Of each primary row pattern variable, its definition, none for TRUE.
type Defines = Vec<Option<Slotted<Cond>>>;

/// [ISO/IEC 9075-2:2023 §7.9 SR4, SR7, SR8, SR18] The row pattern
/// variables of `common`: its primary ones in the order the pattern first
/// names them, then its union ones in order, then the universal one, the
/// union of every primary one.
pub(super) fn variables(common: &PatternCommon) -> Result<Vec<Variable>> {
    let mut primaries: Vec<String> = Vec::new();
    let mut stack = vec![&common.pattern];
    while let Some(p) = stack.pop() {
        match p {
            RowPattern::Variable(name, _) => {
                if !primaries.contains(name) {
                    primaries.push(name.clone());
                }
            }
            RowPattern::Start | RowPattern::End | RowPattern::Empty => {}
            RowPattern::Concat(list) | RowPattern::Alternation(list) | RowPattern::Permute(list) => stack.extend(list.iter().rev()),
            RowPattern::Quantified(q) => stack.push(&q.pattern),
            RowPattern::Excluded(inner) => stack.push(inner),
        }
    }
    let mut variables: Vec<Variable> =
        (0..).zip(&primaries).map(|(k, name)| Variable { name: name.clone(), components: vec![k] }).collect();
    for s in &common.subsets {
        let refused = |text: String| SqlError::at(Code::SyntaxRule, s.at, text);
        if primaries.contains(&s.name) {
            return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.9 SR9] {} is a primary row pattern variable, so no union one", s.name)));
        }
        if variables.iter().any(|v| v.name == s.name) {
            return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.9 SR15] the union row pattern variable {} is defined twice", s.name)));
        }
        let mut components = Vec::new();
        for name in &s.variables {
            let Some(k) = primaries.iter().position(|p| p == name) else {
                return Err(refused(format!(
                    "[ISO/IEC 9075-2:2023 §7.9 SR11] {name}, of SUBSET {}, is no primary row pattern variable",
                    s.name
                )));
            };
            if !components.contains(&k) {
                components.push(k);
            }
        }
        components.sort_unstable();
        variables.push(Variable { name: s.name.clone(), components });
    }
    variables.push(Variable { name: String::new(), components: (0..primaries.len()).collect() });
    Ok(variables)
}

/// [ISO/IEC 9075-2:2023 §7.9 SR19] `p`, its primary row pattern variables
/// numbered as `variables` numbers them, its quantifiers read as the
/// parser normalized them.
pub(super) fn pattern(p: &RowPattern, variables: &[Variable]) -> Result<Pattern> {
    pattern_at(p, variables, 0)
}

/// `pattern`, `depth` deep.
fn pattern_at(p: &RowPattern, variables: &[Variable], depth: usize) -> Result<Pattern> {
    // Deactivated: the parser nests a <row pattern> no deeper than this
    // (RULES.md R-COV-3), so it is bound by no more recursion.
    if depth > MAX_NESTING_DEPTH {
        return Err(SqlError::new(Code::Unsupported, format!("[DESIGN] a <row pattern> nests at most {MAX_NESTING_DEPTH} deep")));
    }
    let next = depth.saturating_add(1);
    let all = |list: &[RowPattern]| list.iter().map(|q| pattern_at(q, variables, next)).collect::<Result<Vec<Pattern>>>();
    Ok(match p {
        RowPattern::Variable(name, at) => {
            let k = variables
                .iter()
                .position(|v| v.name == *name)
                .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {name} is no variable")))
                .or_at(*at)?;
            Pattern::Variable(k)
        }
        RowPattern::Start => Pattern::Start,
        RowPattern::End => Pattern::End,
        RowPattern::Empty => Pattern::Empty,
        RowPattern::Concat(list) => Pattern::Concat(all(list)?),
        RowPattern::Alternation(list) => Pattern::Alternation(all(list)?),
        RowPattern::Permute(list) => Pattern::Permute(all(list)?),
        RowPattern::Excluded(inner) => Pattern::Excluded(Box::new(pattern_at(inner, variables, next)?)),
        // [§7.9 SR20] a quantifier's bounds the parser checked.
        RowPattern::Quantified(q) => {
            let pattern = Box::new(pattern_at(&q.pattern, variables, next)?);
            Pattern::Repeat { pattern, min: q.min, max: q.max, reluctant: q.reluctant }
        }
    })
}

/// [ISO/IEC 9075-2:2023 §7.9 SR23] Where the next match is sought, SKIP
/// PAST LAST ROW if not written, SKIP TO of a row pattern variable SKIP TO
/// LAST of it.
pub(super) fn skip(s: &SkipTo, variables: &[Variable]) -> Result<Skip> {
    let named = |name: &str, at: usize| {
        let union = variables.split_last().map_or(&[][..], |(_, named)| named);
        union.iter().position(|v| v.name == name).ok_or_else(|| {
            SqlError::at(Code::SyntaxRule, at, format!("[ISO/IEC 9075-2:2023 §7.9 SR5] no row pattern variable {name} is in scope"))
        })
    };
    Ok(match s {
        SkipTo::NextRow => Skip::NextRow,
        SkipTo::PastLastRow => Skip::PastLastRow,
        SkipTo::First(name, at) => Skip::First(named(name, *at)?),
        SkipTo::Last(name, at) => Skip::Last(named(name, *at)?),
    })
}

/// [ISO/IEC 9075-2:2023 §7.9 SR10, SR12, SR17] Of each primary row pattern
/// variable of `variables`, its definition of `common`, bound by `binder`
/// with `r`; none for TRUE.
pub(super) fn defines(binder: &Binder<'_>, r: &Recognizing, common: &PatternCommon, variables: &[Variable]) -> Result<Defines> {
    let primaries = variables.len().saturating_sub(common.subsets.len()).saturating_sub(1);
    let mut defines: Defines = std::iter::repeat_with(|| None).take(primaries).collect();
    r.defining(true);
    for d in &common.defines {
        let refused = |text: String| SqlError::at(Code::SyntaxRule, d.at, text);
        let Some(k) = variables.iter().take(primaries).position(|v| v.name == d.name) else {
            return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.9 SR10] {} of DEFINE is no primary row pattern variable", d.name)));
        };
        let Some(slot) = defines.get_mut(k) else { return Err(SqlError::new(Code::Api, "[DESIGN] a definition is of no variable")) };
        if slot.is_some() {
            return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.9 SR12] {} is defined twice", d.name)));
        }
        let expr = binder.search_condition(&d.condition, r.scope());
        let values = r.take_slots();
        *slot = Some(Slotted { expr: expr?, values });
    }
    r.defining(false);
    Ok(defines)
}

/// [ISO/IEC 9075-2:2023 §7.9] The `<row pattern common syntax>` `common`
/// of the variables `variables`, its definitions `defines`, SEEK if
/// `seek`.
pub(super) fn matching(common: &PatternCommon, variables: Vec<Variable>, defines: Defines, seek: bool) -> Result<Matching> {
    let pattern = pattern(&common.pattern, &variables)?;
    let skip = skip(&common.skip, &variables)?;
    // [§9.41] a pattern beyond the matcher's limits is refused as it is
    // bound, not as it is run.
    crate::pattern::Program::new(&pattern)?;
    Ok(Matching { pattern, variables, defines, skip, seek })
}

/// The names of `variables` but the universal one: the primary ones, then
/// the union ones.
pub(super) fn names(variables: &[Variable]) -> Vec<String> {
    variables.split_last().map_or_else(Vec::new, |(_, named)| named.iter().map(|v| v.name.clone()).collect())
}
