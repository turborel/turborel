//! What a definition sees as it was defined (`Horizon`): the columns of
//! the tables it reads, its subject routines, the registered XML Schemas
//! it indicates and the registered XQuery modules its queries import, each
//! bound again as it is used.

use super::{Catalog, QualifiedName};

/// What a definition sees as it was defined, of a view's query, a CHECK's
/// condition, a trigger's action, a routine's body or a generation
/// expression, each bound again as it is used.
///
/// [ISO/IEC 9075:1992 §11.11 GR2, note] The columns it sees of each base
/// table it reads: as many as the table had when it was defined. A column
/// added to a table is its last [§11.11 GR4], so those are the first of
/// them; the query's implicit column references (*, `<qualifier>`.*,
/// TABLE, a NATURAL join's, CORRESPONDING's, an unqualified name's table)
/// "are replaced by explicit `<column reference>`s when the clause is
/// originally evaluated", and a column added changes none. A table it does
/// not list it sees whole.
///
/// [ISO/IEC 9075-2:2023 §4.35.2] The subject routines of its routine
/// invocations, fixed as it was defined: a routine defined later, or one
/// its owner is granted EXECUTE on later, is none of them (RULES.md D73).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Horizon {
    /// Each base table it reads and the columns it sees of it.
    pub widths: Vec<(QualifiedName, usize)>,
    /// The specific names of the routines it invokes, its subject routines;
    /// none of a statement, whose invocations are of the routines there
    /// are as it is bound.
    pub routines: Option<Vec<QualifiedName>>,
    /// [ISO/IEC 9075-14:2006 draft §4.2.6] The registered XML Schemas its
    /// `<XML valid according to what>`s indicated as it was defined, the
    /// ones they indicate whenever it is bound again; each once, in order.
    pub xml_schemas: Vec<QualifiedName>,
    /// \[EXTENSION\] RULES.md D106 The registered XQuery modules its
    /// queries imported as it was defined [XQuery §4.11], the ones they
    /// import whenever it is bound again; each once, in order.
    pub xquery_modules: Vec<QualifiedName>,
}

/// What a definition's binding notes it indicates and imports, for its
/// horizon to keep: the registered XML Schemas [ISO/IEC 9075-14:2006 draft
/// §4.2.6] and the registered XQuery modules (RULES.md D106).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indicated {
    /// The registered XML Schemas.
    pub xml_schemas: Vec<QualifiedName>,
    /// The registered XQuery modules.
    pub xquery_modules: Vec<QualifiedName>,
}

/// No horizon at all: every table seen whole, every routine there is a
/// candidate.
pub static WHOLE: Horizon = Horizon { widths: Vec::new(), routines: None, xml_schemas: Vec::new(), xquery_modules: Vec::new() };

impl Horizon {
    /// The degree of each base table of `tables` in `catalog` now.
    pub fn of(catalog: &dyn Catalog, tables: &[QualifiedName]) -> Horizon {
        let widths = tables.iter().filter_map(|t| catalog.table(t).map(|d| (t.clone(), d.columns.len()))).collect();
        Horizon { widths, routines: None, xml_schemas: Vec::new(), xquery_modules: Vec::new() }
    }

    /// It, of the widths `widths` in place of its own.
    #[must_use]
    pub fn of_widths(&self, widths: Vec<(QualifiedName, usize)>) -> Horizon {
        Horizon { widths, ..self.clone() }
    }

    /// It with `invoked`, the specific names of the routines its definition
    /// invokes, as its subject routines, once each.
    #[must_use]
    pub fn invoking(mut self, mut invoked: Vec<QualifiedName>) -> Horizon {
        invoked.sort();
        invoked.dedup();
        self.routines = Some(invoked);
        self
    }

    /// It with `indicated`: the registered XML Schemas its definition's
    /// `<XML valid according to what>`s indicate [ISO/IEC 9075-14:2006
    /// draft §4.2.6], and the registered XQuery modules its queries
    /// import, once each.
    #[must_use]
    pub fn indicating(mut self, indicated: Indicated) -> Horizon {
        let Indicated { mut xml_schemas, mut xquery_modules } = indicated;
        for names in [&mut xml_schemas, &mut xquery_modules] {
            names.sort();
            names.dedup();
        }
        self.xml_schemas = xml_schemas;
        self.xquery_modules = xquery_modules;
        self
    }

    /// The subject routines of its invocations; none of a statement's.
    pub fn subjects(&self) -> Option<&[QualifiedName]> {
        self.routines.as_deref()
    }

    /// [ISO/IEC 9075-14:2006 draft §4.2.6] The registered XML Schemas a URI
    /// may indicate as it is bound: of a definition's, the ones indicated
    /// as it was defined; none of a statement's, of which any may be.
    pub fn pins(&self) -> &[QualifiedName] {
        if self.routines.is_some() { &self.xml_schemas } else { &[] }
    }

    /// What a definition kept to import and validate by: of a
    /// definition's, the registered XML Schemas and XQuery modules it
    /// noted as it was defined, which alone its queries may import as it is
    /// bound again; none of a statement's, of which any may be.
    pub fn kept(&self) -> Option<(&[QualifiedName], &[QualifiedName])> {
        self.routines.is_some().then_some((self.xml_schemas.as_slice(), self.xquery_modules.as_slice()))
    }

    /// How many columns of `table` are seen, if not all.
    pub fn width(&self, table: &QualifiedName) -> Option<usize> {
        self.widths.iter().find(|(t, _)| t == table).map(|(_, w)| *w)
    }

    /// It once the column at `at` of `table` is dropped [§11.15 GR4, GR6]: one
    /// fewer seen, if the column was.
    #[must_use]
    pub fn without(&self, table: &QualifiedName, at: usize) -> Horizon {
        let widths = self.widths.iter().map(|(t, w)| (t.clone(), if t == table && at < *w { w.saturating_sub(1) } else { *w })).collect();
        self.of_widths(widths)
    }
}
