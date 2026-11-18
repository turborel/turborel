//! \[EXTENSION\] RULES.md D106: XML index lookups for the conjuncts of a
//! table's filter that keep only rows whose query's value is not empty:
//! XMLEXISTS, XMLQUERY NULL ON EMPTY IS NOT NULL, and XMLTABLE's row
//! pattern, implied where XMLTABLE joins the rows it is of (`implied`),
//! each of a query reduced to an index's (`xquery::Reduction`) of the
//! column. A value of XML(SEQUENCE) may be no one document node, and the
//! query raise an error of it as its context item [ISO/IEC 9075-14:2006
//! draft §6.18 GR1a ii 2 B III]: an index gives the row of every such
//! value, so a lookup of such a column hides none, but its query is never
//! answered by the index, nor free of errors (`functions::xml`).
//!
//! A query's rows are among those each of its lookups gives
//! (`XmlQuery::lookups`), its own and its filters', so a probe reads the
//! cheapest and those the index counts, their rows each lookup gives; of
//! several such conjuncts of the column, joined by AND, the lookups of all
//! [Pal et al. §3.3]. A query that may raise an error raises it of a row
//! one of its raising lookups gives (`Reduction::raising`), which gives
//! every row the query holds of too: a probe of such a query reads each of
//! those, its rows those any gives.
//!
//! An index changes no result: a conjunct used stays in the filter, which
//! checks each row fetched, as of a function lookup (D34); an implied one
//! is checked by XMLTABLE itself, which gives no row of a value the lookup
//! does not select. And it hides no error (D15): the rows a probe leaves
//! out are those its queries give the empty sequence of with no error, so
//! it is used only if every other conjunct cannot raise one, or is another
//! such query that raises none.

use std::sync::Arc;

use super::Candidate;
use crate::catalog::IndexDesc;
use crate::functions::XmlFilter;
use crate::index::xml::{MAX_LOOKUPS, XmlCost, XmlLookup, XmlProbe};
use crate::index::{Index, Probe, XML_PRIMARY_METHOD};
use crate::logical::{Cond, Expr};
use crate::stats::Statistics;
use crate::storage::cost_of;
use crate::xquery::Reduction;

/// A conjunct an XML index may find the rows of: its query's context
/// item's column, and its reduction.
struct Reduced {
    /// The column of its context item.
    column: usize,
    /// Its query reduced.
    reduction: Reduction,
    /// Whether it is the query's test alone, so an index answering the
    /// query answers it: XMLEXISTS, or an implied filter.
    answerable: bool,
}

/// The column of a call's one argument, the context item: a column, or
/// its copy, passed BY VALUE.
fn context_column(args: &[Expr]) -> Option<usize> {
    let [argument] = args else { return None };
    if let Expr::Column(column) = argument {
        return Some(*column);
    }
    let Expr::Call(f, inner) = argument else { return None };
    let (Some(XmlFilter::Copy), [Expr::Column(column)]) = (f.0.xml_filter(), inner.as_slice()) else { return None };
    Some(*column)
}

/// The reduction of `e`, a value null exactly when its query's value is
/// empty: XMLQUERY NULL ON EMPTY, as it is or made a document node, as
/// RETURNING CONTENT makes it.
fn null_of_empty(e: &Expr) -> Option<Reduced> {
    let Expr::Call(f, args) = e else { return None };
    let (filter, args) = match f.0.xml_filter()? {
        XmlFilter::KeepsNull => {
            let [Expr::Call(g, inner)] = args.as_slice() else { return None };
            (g.0.xml_filter()?, inner)
        }
        other @ (XmlFilter::NullOfEmpty(_) | XmlFilter::Exists(_) | XmlFilter::Implied(_) | XmlFilter::NoRows(..) | XmlFilter::Copy) => {
            (other, args)
        }
    };
    let XmlFilter::NullOfEmpty(reduction) = filter else { return None };
    Some(Reduced { column: context_column(args)?, reduction, answerable: false })
}

/// The reduction of `c`, if it keeps a row only if its query's value is
/// not empty, and whether it is implied; none if not.
fn reduced(c: &Cond) -> Option<(Reduced, bool)> {
    if let Cond::IsNull(e, true) = c {
        return null_of_empty(e).map(|r| (r, false));
    }
    let Cond::Value(Expr::Call(f, args)) = c else { return None };
    let filter = f.0.xml_filter()?;
    let implied = matches!(filter, XmlFilter::Implied(_));
    let (XmlFilter::Exists(reduction) | XmlFilter::Implied(reduction)) = filter else { return None };
    Some((Reduced { column: context_column(args)?, reduction, answerable: true }, implied))
}

/// Whether `c` is an implied filter (`XmlFilter::Implied`), which no
/// row is checked by.
pub(super) fn implied(c: &Cond) -> bool {
    reduced(c).is_some_and(|(_, implied)| implied)
}

/// Whether `c` raises no error of a row a lookup leaves out: it cannot
/// raise one, or it is a query, of a value of one node, that raises none.
fn raises_none(c: &Cond) -> bool {
    !c.can_raise() || reduced(c).is_some_and(|(r, _)| r.reduction.raise_free)
}

/// The XML queries of `ix` that find the rows of a conjunct of `conds`,
/// or of an implied filter of `implied`, each priced by the index `found`
/// against the table's `all` rows: of each conjunct of a column the index
/// keys, the others raising no error of a row it leaves out; and of all of
/// them together, if there are several. Of the primary, a query the
/// conjunct is the test of, exactly, is answered by it
/// (`Index::xml_answers`), and the conjunct no longer a filter's.
pub(super) fn candidates((conds, implied): (&[Cond], &[Cond]), ix: &IndexDesc, found: &Arc<dyn Index>, all: f64) -> Vec<Candidate> {
    let [key] = ix.columns.as_slice() else { return Vec::new() };
    let each = conds.iter().enumerate().map(|(at, c)| (Some(at), c)).chain(implied.iter().map(|c| (None, c)));
    let of_key: Vec<(Option<usize>, Reduced)> =
        each.filter_map(|(at, c)| reduced(c).map(|(r, _)| (at, r))).filter(|(_, r)| r.column == *key).collect();
    // D15: the others raise no error of a row the probe leaves out.
    let others_raise_none = |used: &[Option<usize>]| conds.iter().enumerate().all(|(j, c)| used.contains(&Some(j)) || raises_none(c));
    let mut out: Vec<Candidate> = of_key
        .iter()
        .filter(|(at, _)| others_raise_none(&[*at]))
        .filter_map(|one| candidate(std::slice::from_ref(one), ix, found, all))
        .collect();
    let ats: Vec<Option<usize>> = of_key.iter().map(|(at, _)| *at).collect();
    if of_key.len() > 1 && others_raise_none(&ats) {
        out.extend(candidate(&of_key, ix, found, all));
    }
    out
}

/// The probe of `of`, conjuncts reduced, of `ix`, priced by the index
/// `found` against the table's `all` rows; none if it prices a lookup of
/// none, or one of them may raise an error of more lookups than a probe
/// reads.
fn candidate(of: &[(Option<usize>, Reduced)], ix: &IndexDesc, found: &Arc<dyn Index>, all: f64) -> Option<Candidate> {
    let raising: Vec<XmlLookup> = of
        .iter()
        .flat_map(|(_, r)| {
            let all_of = r.reduction.query.lookups();
            r.reduction.raising.iter().filter_map(move |&n| all_of.get(n).cloned()).collect::<Vec<_>>()
        })
        .collect();
    let any = !raising.is_empty();
    let asked = if any { raising } else { of.iter().flat_map(|(_, r)| r.reduction.query.lookups()).collect() };
    let mut costed = Vec::with_capacity(asked.len());
    for lookup in asked {
        // An index that prices no lookup answers none (`Index::xml_cost`).
        let entries = Arc::clone(found).xml_cost(&lookup)?.entries;
        costed.push((lookup, entries));
    }
    let (lookups, guess) = if any { union_of(costed, all)? } else { intersection_of(costed, all) };
    // The primary reads each row it holds of a value, a seek at least,
    // which its count of the entries read does not see: it is guessed to
    // give no fewer rows than its queries' own lookups count.
    let guess = if ix.method == XML_PRIMARY_METHOD { guess.max(own_lookups(of, found, all)?) } else { guess };
    let [(at, one)] = of else {
        let queries = of.iter().map(|(_, r)| r.reduction.query.clone()).collect();
        let probe = Probe::Xml(Box::new(XmlProbe { queries, answered: false, lookups, any }));
        return Some(Candidate::one(probe, (of.iter().filter_map(|(at, _)| *at).collect(), 0, guess, true)));
    };
    let answered = one.answerable && one.reduction.exact && ix.method == XML_PRIMARY_METHOD;
    let probe = Probe::Xml(Box::new(XmlProbe { queries: vec![one.reduction.query.clone()], answered, lookups, any }));
    // D34, D106: the filter keeps the conjunct, to check each row, but of
    // a query the index answers.
    Some(Candidate::one(probe, (at.iter().copied().collect(), 0, guess, true)))
}

/// The rows the queries' own lookups of `of` give, of the table's `all`,
/// by the index `found`'s count: the most of them; none if it prices one
/// of none.
fn own_lookups(of: &[(Option<usize>, Reduced)], found: &Arc<dyn Index>, all: f64) -> Option<f64> {
    let mut most = 0.0_f64;
    for (_, r) in of {
        let entries = Arc::clone(found).xml_cost(&r.reduction.query.lookup)?.entries;
        most = most.max(entries.map_or(all, |e| cost_of(e).min(all)));
    }
    Some(most)
}

/// The lookups of a probe of the rows any of `costed` gives, and the rows
/// it is guessed to give of the table's `all`: the entries of each, if
/// each is counted; none if more than a probe reads.
fn union_of(costed: Vec<(XmlLookup, Option<usize>)>, all: f64) -> Option<(Vec<XmlLookup>, f64)> {
    if costed.len() > MAX_LOOKUPS {
        return None;
    }
    let entries = costed.iter().try_fold(0_usize, |n, (_, e)| e.map(|e| n.saturating_add(e)));
    Some((costed.into_iter().map(|(l, _)| l).collect(), entries.map_or(all, |e| cost_of(e).min(all))))
}

/// The lookups of a probe of the rows every one of `costed` gives, and the
/// rows it is guessed to give of the table's `all`: the cheapest first,
/// then each the index counted, to `MAX_LOOKUPS`, as a lookup it did not
/// count would read more than it could narrow; each a share of the
/// table's rows, of its entries, as if apart, but a row at least if the
/// cheapest gives one.
fn intersection_of(mut costed: Vec<(XmlLookup, Option<usize>)>, all: f64) -> (Vec<XmlLookup>, f64) {
    costed.sort_by_key(|(_, entries)| entries.unwrap_or(usize::MAX));
    let kept: Vec<(XmlLookup, Option<usize>)> =
        costed.into_iter().enumerate().filter(|(n, (_, e))| *n == 0 || e.is_some()).take(MAX_LOOKUPS).map(|(_, c)| c).collect();
    let share = |e: usize| (cost_of(e) / all.max(1.0_f64)).min(1.0_f64);
    let guess = kept.iter().filter_map(|(_, e)| *e).fold(all, |rows, e| rows * share(e));
    // A row at least, if the cheapest gives one.
    let least = kept.first().and_then(|(_, e)| *e).map_or(1.0_f64, |e| cost_of(e).min(1.0_f64));
    (kept.into_iter().map(|(l, _)| l).collect(), guess.max(least))
}

/// Whether `probes` are an XML query an index answers (`XmlProbe`).
pub(super) fn answered(probes: &[Probe]) -> bool {
    let Some(Probe::Xml(asked)) = probes.first() else { return false };
    asked.answered
}

/// What checking the XML queries of `conds` costs of a row, each a pass
/// over a value of its column's average width as `stats` know it
/// (`XmlCost::of_check`), its pages each a level of an index's descent,
/// `descent`.
pub(super) fn check_cost(conds: &[Cond], stats: Option<&dyn Statistics>, descent: f64) -> f64 {
    let pages = |column: usize| {
        let width = stats.and_then(|s| s.width(&[column])).unwrap_or(0.0_f64);
        let octets = format!("{:.0}", width.max(0.0_f64).ceil()).parse::<u64>().unwrap_or(u64::MAX);
        XmlCost::of_check(octets).pages.unwrap_or(1.0_f64)
    };
    conds.iter().filter_map(reduced).map(|(r, _)| pages(r.column) * descent).sum()
}
