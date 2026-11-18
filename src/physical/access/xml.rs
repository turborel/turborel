//! \[EXTENSION\] RULES.md D106: XML index lookups for the conjuncts of a
//! table's filter that keep only rows whose query's value is not empty:
//! XMLEXISTS, XMLQUERY NULL ON EMPTY IS NOT NULL, and XMLTABLE's row
//! pattern, implied where XMLTABLE joins the rows it is of (`implied`),
//! each of a query reduced to a lookup (`xquery::Reduction`) of the
//! column. A value of XML(SEQUENCE) may be no one document node, and the
//! query raise an error of it as its context item [ISO/IEC 9075-14:2006
//! draft §6.18 GR1a ii 2 B III]: an index gives the row of every such
//! value, so a lookup of such a column hides none, but its query is never
//! answered by the index, nor free of errors (`functions::xml`).
//!
//! An index changes no result: a conjunct used stays in the filter, which
//! checks each row fetched, as of a function lookup (D34); an implied one
//! is checked by XMLTABLE itself, which gives no row of a value the lookup
//! does not select. And it hides no error (D15): the rows a lookup leaves
//! out are those its own query gives the empty sequence of with no error,
//! so it is used only if every other conjunct cannot raise one, or is
//! another such query that raises none.

use std::sync::Arc;

use super::Candidate;
use crate::catalog::IndexDesc;
use crate::functions::XmlFilter;
use crate::index::xml::{XmlCost, XmlProbe};
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
/// or of an implied filter of `implied`, each priced by the index
/// `found` against the table's `all` rows: of a conjunct of a column the
/// index keys, the others raising no error of a row it leaves out. Of
/// the primary, a query the conjunct is the test of, exactly, is answered
/// by it (`Index::xml_answers`), and the conjunct no longer a filter's.
pub(super) fn candidates((conds, implied): (&[Cond], &[Cond]), ix: &IndexDesc, found: &Arc<dyn Index>, all: f64) -> Vec<Candidate> {
    let [key] = ix.columns.as_slice() else { return Vec::new() };
    let mut out = Vec::new();
    let each = conds.iter().enumerate().map(|(at, c)| (Some(at), c)).chain(implied.iter().map(|c| (None, c)));
    for (at, c) in each {
        let Some((r, _)) = reduced(c) else { continue };
        if r.column != *key {
            continue;
        }
        let mut others = conds.iter().enumerate().filter(|(j, _)| Some(*j) != at);
        if !others.all(|(_, other)| raises_none(other)) {
            continue;
        }
        // An index that prices no lookup answers none (`Index::xml_cost`).
        let Some(cost) = Arc::clone(found).xml_cost(&r.reduction.query.lookup) else { continue };
        let guess = cost.entries.map_or(all, |entries| cost_of(entries).min(all));
        let answered = r.answerable && r.reduction.exact && ix.method == XML_PRIMARY_METHOD;
        let probe = Probe::Xml(Box::new(XmlProbe { query: r.reduction.query, answered }));
        // D34, D106: the filter keeps the conjunct, to check each row, but
        // of a query the index answers.
        out.push(Candidate::one(probe, (at.into_iter().collect(), 0, guess, true)));
    }
    out
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
