//! \[EXTENSION\] A GRAPH_TABLE, the `<table primary>` [ISO/IEC 9075-2:2023
//! §7.6] that matches a graph pattern in a property graph and gives a row
//! of its COLUMNS per match, per node or per step of a path.

use super::{GraphPattern, GraphVariable};
use crate::ast::{SearchCondition, Subquery, SystemTimeKind, SystemTimeRead, TableName, ValueExpression};

/// GRAPH_TABLE ( `<graph reference>` MATCH `<graph pattern>` \[`<rows
/// clause>`\] COLUMNS ( `<graph column>` \[, ...\] ) ) \[\[AS\]
/// `<correlation name>`\] \[FOR SYSTEM_TIME ... | AS OF TIMESTAMP
/// `<value expression>`\].
#[derive(Debug, Clone)]
pub struct GraphTable {
    /// The property graph.
    pub graph: GraphReference,
    /// The pattern MATCH gives.
    pub pattern: GraphPattern,
    /// Which rows a match gives.
    pub rows: GraphRows,
    /// The columns, in order, one at least.
    pub columns: Vec<GraphColumn>,
    /// The `<correlation name>`, if written.
    pub correlation: Option<String>,
    /// The point in time its element tables are read at, if written.
    pub system_time: Option<Box<SystemTimeRead>>,
    /// Where GRAPH_TABLE is written.
    pub at: usize,
}

/// The graph a GRAPH_TABLE matches its pattern in.
#[derive(Debug, Clone)]
pub enum GraphReference {
    /// A property graph's name, perhaps of a schema.
    Named(TableName),
    /// \[EXTENSION\] LINE_GRAPH ( `<graph name>` , `<label>` ): the graph
    /// whose nodes are the named graph's edges of the label, an edge from
    /// each to each that follows it.
    LineGraph {
        /// The graph.
        graph: TableName,
        /// The edges' label.
        label: String,
        /// Where LINE_GRAPH is written.
        at: usize,
    },
}

impl GraphReference {
    /// The property graph it names.
    #[must_use]
    pub fn name(&self) -> &TableName {
        match self {
            GraphReference::Named(n) | GraphReference::LineGraph { graph: n, .. } => n,
        }
    }
}

/// The rows each match gives.
#[derive(Debug, Clone)]
pub enum GraphRows {
    /// ONE ROW PER MATCH, also when none is written.
    Match,
    /// ONE ROW PER VERTEX ( `<variable>` ): a row per node of the path,
    /// the variable bound to it.
    Vertex(GraphVariable),
    /// ONE ROW PER STEP ( `<variable>` , `<variable>` , `<variable>` ): a
    /// row per edge of the path, bound to the second, its nodes before and
    /// after to the first and third.
    Step(GraphVariable, GraphVariable, GraphVariable),
}

impl GraphRows {
    /// The variables it declares.
    #[must_use]
    pub fn variables(&self) -> Vec<&GraphVariable> {
        match self {
            GraphRows::Match => Vec::new(),
            GraphRows::Vertex(v) => vec![v],
            GraphRows::Step(a, e, b) => vec![a, e, b],
        }
    }
}

/// One item of COLUMNS.
#[derive(Debug, Clone)]
pub enum GraphColumn {
    /// `<value expression>` \[AS `<column name>`\].
    Value {
        /// The value.
        value: ValueExpression,
        /// The name, if written.
        name: Option<String>,
        /// Where the value begins.
        at: usize,
    },
    /// `<variable>`.*: every property of every label the element may
    /// have.
    All(GraphVariable),
}

impl GraphTable {
    /// Calls `f` with each subquery of its conditions and values, not
    /// those nested in them.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        let mut conditions: Vec<&SearchCondition> = Vec::new();
        let mut values: Vec<&ValueExpression> = Vec::new();
        self.pattern.operands(&mut conditions, &mut values);
        for column in &self.columns {
            if let GraphColumn::Value { value, .. } = column {
                values.push(value);
            }
        }
        if let Some(t) = &self.system_time {
            match &t.kind {
                SystemTimeKind::AsOf(v) => values.push(v),
                SystemTimeKind::Between(_, a, b) | SystemTimeKind::FromTo(a, b) => values.extend([a, b]),
            }
        }
        for c in conditions {
            c.for_each_subquery(f);
        }
        for v in values {
            v.for_each_subquery(f);
        }
    }
}

impl GraphPattern {
    /// Adds to `conditions` and `values` the search conditions and value
    /// expressions written in it: its prefilters and postfilter, its
    /// properties' values and its selectors' costs.
    pub fn operands<'a>(&'a self, conditions: &mut Vec<&'a SearchCondition>, values: &mut Vec<&'a ValueExpression>) {
        conditions.extend(&self.where_clause);
        let mut todo: Vec<&super::PathExpression> = Vec::new();
        for path in &self.paths {
            prefix_cost(path.prefix.as_ref(), values);
            todo.push(&path.expression);
        }
        prefix_cost(self.keep.as_ref(), values);
        conditions.extend(self.segments.iter().map(|s| &s.condition));
        while let Some(expression) = todo.pop() {
            for factor in expression.terms().iter().flat_map(|t| &t.factors) {
                let element = match &factor.primary {
                    super::PathPrimary::Node(e) => e,
                    super::PathPrimary::Edge(e) => &e.filler,
                    super::PathPrimary::Parenthesized(p) => {
                        conditions.extend(&p.where_clause);
                        prefix_cost(p.prefix.as_ref(), values);
                        todo.push(&p.expression);
                        continue;
                    }
                };
                conditions.extend(&element.where_clause);
                values.extend(element.properties.iter().map(|p| &p.value));
            }
        }
    }
}

/// Adds the COST expression of `prefix`'s selector to `values`, if it
/// has one.
fn prefix_cost<'a>(prefix: Option<&'a super::PathPrefix>, values: &mut Vec<&'a ValueExpression>) {
    if let Some(super::Selector::Cheapest { cost: Some(cost), .. }) = prefix.and_then(|p| p.selector.as_ref()) {
        values.push(cost);
    }
}
