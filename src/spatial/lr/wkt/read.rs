//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14.1 to §15.14.4]
//! Reading linear referencing well-known text: each production produces
//! its value through the constructor its Description names, so the
//! constructor's exceptions are the text's.

use super::lex::Lexer;
use super::{
    DEFAULT, DISEXP, E, EMPTY, FID, FROM, L, LATERALOFFSET, LEID, LETYPE, LINEARELEMENT, LOCATION, LRCURVE, LRDIRECTEDEDGE, LRFEATURE, LRM,
    LRMCONSTRAINTS, LRMID, LRMNAME, LRMOFFSETUOM, LRMTYPE, LRMUOM, MEASURE, NAME, NULL, POSEXP, POSITION, POSLATOFFDIR, POSVEROFFDIR,
    REFERENTS, STARTVALUES, TOWARDS, TYPE, VECTOR, VECTOROFFSETS, VERTICALOFFSET, Z, read_geometry,
};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Geom};
use crate::spatial::lr::model::{
    DistanceExp, LatOffsetExp, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrMeasure, Lrm, PositionExp, Referent, StartValue,
    Vector, VectorOffsetExp, VerOffsetExp,
};
use crate::spatial::lr::{Catalog, EdgeParts, ElementParts, LrValue, LrmParts, MAX_VECTORS, OffsetParts, ReferentNames, deeper, failed};

/// The value whose text is the whole of `text`.
///
/// # Errors
///
/// As `wkt::from_text`.
pub(crate) fn value(text: &str, cat: &Catalog<'_>) -> Result<LrValue> {
    let mut r = Read { lx: Lexer::new(text), cat };
    let v = r.any()?;
    r.lx.finish()?;
    Ok(v)
}

/// A reader of text, with the catalog its constructors check against.
struct Read<'t, 'c> {
    /// The tokens.
    lx: Lexer<'t>,
    /// The catalog.
    cat: &'c Catalog<'c>,
}

/// [§15.14.2 Description 1a] What a `<linear element text body>` produces.
enum ElementText {
    /// An INTEGER, of a `<linear element id representation>`.
    Id(i32),
    /// A value, of a `<linear element representation>`.
    Element(Box<LinearElement>),
}

/// [§15.14.3 Description 1c] What an `<lrm text body>` produces.
enum LrmText {
    /// An INTEGER, of an `<lrm id representation>`.
    Id(i32),
    /// A value, of an `<lrm representation>`.
    Lrm(Lrm),
}

impl Read<'_, '_> {
    /// A value of any type, by its first key word.
    fn any(&mut self) -> Result<LrValue> {
        if self.lx.peek() == Some('(') {
            return self.parenthesized();
        }
        let word = self.lx.peek_word().to_ascii_uppercase();
        Ok(match word.as_str() {
            POSEXP => LrValue::PositionExp(self.posexp(0)?),
            LINEARELEMENT => {
                self.lx.expect(LINEARELEMENT)?;
                match self.element_body(0)? {
                    ElementText::Element(e) => LrValue::LinearElement(*e),
                    ElementText::Id(_) => return Err(self.lx.invalid("LINEARELEMENT LEID alone gives an INTEGER, no linear element")),
                }
            }
            LRM => {
                self.lx.expect(LRM)?;
                match self.lrm_body()? {
                    LrmText::Lrm(l) => LrValue::Lrm(l),
                    LrmText::Id(_) => return Err(self.lx.invalid("LRM LRMID alone gives an INTEGER, no LRM")),
                }
            }
            DISEXP => LrValue::DistanceExp(self.disexp(0)?),
            MEASURE => LrValue::LrMeasure(self.measure_representation()?),
            LATERALOFFSET => LrValue::LatOffsetExp(self.lateral()?),
            VERTICALOFFSET => LrValue::VerOffsetExp(self.vertical()?),
            VECTOROFFSETS => {
                self.lx.expect(VECTOROFFSETS)?;
                LrValue::VectorOffsetExp(self.vectors()?)
            }
            _ => return Err(self.lx.invalid("POSEXP, LINEARELEMENT, LRM or DISEXP expected")),
        })
    }

    /// \[EXTENSION\] A `<start value>` or a `<referent>`, by the key word
    /// after its parenthesis.
    fn parenthesized(&mut self) -> Result<LrValue> {
        let mut ahead = Lexer::new(self.lx.remaining());
        ahead.expect_char('(')?;
        if ahead.peek_word().eq_ignore_ascii_case(LRMID) {
            return Ok(LrValue::StartValue(self.start_value()?));
        }
        Ok(LrValue::Referent(self.referent(0)?))
    }

    /// [§15.14.1] `<position expression text representation>`.
    fn posexp(&mut self, depth: usize) -> Result<PositionExp> {
        let next = deeper(depth)?;
        self.lx.expect(POSEXP)?;
        self.lx.expect_char('(')?;
        self.lx.expect(LINEARELEMENT)?;
        let element = self.element_body(next)?;
        self.lx.expect_char(')')?;
        self.lx.expect_char(',')?;
        self.lx.expect_char('(')?;
        self.lx.expect(LRM)?;
        let lrm = self.lrm_body()?;
        self.lx.expect_char(')')?;
        self.lx.expect_char(',')?;
        self.lx.expect_char('(')?;
        let d = self.disexp(next)?;
        self.lx.expect_char(')')?;
        Ok(match (element, lrm) {
            (ElementText::Id(leid), LrmText::Id(lrmid)) => PositionExp::of_ids(leid, lrmid, d),
            (ElementText::Id(leid), LrmText::Lrm(l)) => PositionExp::of_id_and_lrm(leid, l, d),
            (ElementText::Element(e), LrmText::Id(lrmid)) => PositionExp::of_element_and_id(*e, lrmid, d),
            (ElementText::Element(e), LrmText::Lrm(l)) => PositionExp::of_element_and_lrm(*e, l, d),
        })
    }

    /// [§15.14.2] `<linear element text body>`, after LINEARELEMENT.
    fn element_body(&mut self, depth: usize) -> Result<ElementText> {
        let next = deeper(depth)?;
        if self.lx.eat(LEID) {
            return Ok(ElementText::Id(self.lx.integer()?));
        }
        if self.lx.eat(LRFEATURE) {
            let parts = self.element_text()?;
            self.lx.expect(FID)?;
            let fid = self.lx.text()?;
            let referents = if self.lx.eat(REFERENTS) { Some(self.referents(next)?) } else { None };
            return Ok(ElementText::Element(Box::new(LinearElement::Feature(LrFeature::new_with_referents(parts, Some(fid), referents)?))));
        }
        if self.lx.eat(LRCURVE) {
            let parts = self.element_text()?;
            let curve = read_geometry(self.lx.geometry_text()?)?;
            return Ok(ElementText::Element(Box::new(LinearElement::Curve(LrCurve::new(parts, Some(curve))?))));
        }
        self.lx.expect(LRDIRECTEDEDGE)?;
        let parts = self.element_text()?;
        let topology_type = if self.lx.eat(E) {
            'E'
        } else {
            self.lx.expect(L)?;
            'L'
        };
        let topology_name = self.lx.text()?;
        let edge_id = self.lx.integer()?;
        let edge = EdgeParts { topology_type: Some(topology_type), topology_name: Some(topology_name), edge_id: Some(edge_id) };
        Ok(ElementText::Element(Box::new(LinearElement::Edge(LrDirectedEdge::new(parts, edge)?))))
    }

    /// [§15.14.2] `<linear element text>` and the comma after it.
    fn element_text(&mut self) -> Result<ElementParts> {
        self.lx.expect(LEID)?;
        let id = self.lx.integer()?;
        self.lx.expect_char(',')?;
        self.lx.expect(DEFAULT)?;
        self.lx.expect(LRMID)?;
        let default_lrm = self.lx.integer()?;
        self.lx.expect_char(',')?;
        self.lx.expect(DEFAULT)?;
        let default_measure = self.measure_representation()?;
        self.lx.expect_char(',')?;
        self.lx.expect(LETYPE)?;
        let element_type = self.lx.text_or_null()?;
        self.lx.expect_char(',')?;
        let mut start_values = None;
        if self.lx.eat(STARTVALUES) {
            let values = self.list(Self::start_value)?;
            start_values = Some(values);
            self.lx.expect_char(',')?;
        }
        Ok(ElementParts {
            id: Some(id),
            default_lrm: Some(default_lrm),
            default_measure: Some(default_measure),
            element_type,
            start_values,
        })
    }

    /// `( item {, item}... )`, or \[EXTENSION\] `()` of an empty ARRAY.
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        self.lx.expect_char('(')?;
        let mut out = Vec::new();
        if self.lx.eat_char(')') {
            return Ok(out);
        }
        loop {
            out.push(item(self)?);
            if !self.lx.eat_char(',') {
                break;
            }
        }
        self.lx.expect_char(')')?;
        Ok(out)
    }

    /// [§15.14.3 Description 1a] `<start value>`.
    fn start_value(&mut self) -> Result<StartValue> {
        self.lx.expect_char('(')?;
        self.lx.expect(LRMID)?;
        let lrm = self.lx.integer()?;
        self.lx.expect_char(',')?;
        let measure = self.measure_text()?;
        self.lx.expect_char(')')?;
        Ok(StartValue::new(lrm, measure))
    }

    /// [§15.14.2] `<measure representation>`.
    fn measure_representation(&mut self) -> Result<LrMeasure> {
        self.lx.expect(MEASURE)?;
        self.measure_text()
    }

    /// [§15.14.3 Description 1b] `<measure text>`.
    fn measure_text(&mut self) -> Result<LrMeasure> {
        let measure = self.lx.number()?;
        let unit = self.lx.unit()?;
        LrMeasure::new_with_units(Some(measure), unit, self.cat.units)
    }

    /// [§15.14.2] `<referents>`, after REFERENTS.
    fn referents(&mut self, depth: usize) -> Result<Vec<Referent>> {
        self.list(|r| r.referent(depth))
    }

    /// [§15.14.2 Description 1o] `<referent>`.
    fn referent(&mut self, depth: usize) -> Result<Referent> {
        let next = deeper(depth)?;
        self.lx.expect_char('(')?;
        self.lx.expect(NAME)?;
        let name = self.lx.text()?;
        self.lx.expect_char(',')?;
        self.lx.expect(TYPE)?;
        let referent_type = self.lx.text()?;
        let mut position = None;
        let mut location = None;
        if self.lx.eat_char(',') {
            if self.lx.eat(POSITION) {
                position = Some(self.point()?);
                if self.lx.eat_char(',') {
                    self.lx.expect(LOCATION)?;
                    location = Some(self.posexp(next)?);
                }
            } else {
                self.lx.expect(LOCATION)?;
                location = Some(self.posexp(next)?);
            }
        }
        self.lx.expect_char(')')?;
        Referent::new(Some(name), Some(referent_type), position, location, self.cat)
    }

    /// [§15.14.2] `<point text representation>`.
    fn point(&mut self) -> Result<Geom> {
        let g = read_geometry(self.lx.geometry_text()?)?;
        if matches!(g.body, Body::Point(_)) {
            return Ok(g);
        }
        Err(self.lx.invalid("a referent's position is not a point"))
    }

    /// [§15.14.3] `<lrm text body>`, after LRM.
    fn lrm_body(&mut self) -> Result<LrmText> {
        self.lx.expect(LRMID)?;
        let id = self.lx.integer()?;
        if !self.lx.eat_char(',') {
            return Ok(LrmText::Id(id));
        }
        self.lx.expect(LRMNAME)?;
        let name = self.lx.text()?;
        self.lx.expect_char(',')?;
        self.lx.expect(LRMTYPE)?;
        let lrm_type = self.lx.text()?;
        self.lx.expect_char(',')?;
        self.lx.expect(LRMUOM)?;
        let units = self.lx.text()?;
        let mut parts = LrmParts { id: Some(id), name: Some(name), lrm_type: Some(lrm_type), units: Some(units), constraints: None };
        let mut offsets = OffsetParts::default();
        if self.lx.eat_char(',') {
            if self.lx.eat(LRMCONSTRAINTS) {
                parts.constraints = Some(self.list(|r| r.lx.text())?);
                if !self.lx.eat_char(',') {
                    return Ok(LrmText::Lrm(Lrm::new_with_offsets(parts, offsets, self.cat.units)?));
                }
            }
            offsets = self.offset_attributes()?;
        }
        Ok(LrmText::Lrm(Lrm::new_with_offsets(parts, offsets, self.cat.units)?))
    }

    /// [§15.14.3] `<lrm offset attributes>`.
    fn offset_attributes(&mut self) -> Result<OffsetParts> {
        self.lx.expect(LRMOFFSETUOM)?;
        let mut offsets = OffsetParts { units: Some(self.lx.text()?), lateral: None, vertical: None };
        if self.lx.eat_char(',') {
            if self.lx.eat(POSLATOFFDIR) {
                offsets.lateral = Some(self.lx.text()?);
                if !self.lx.eat_char(',') {
                    return Ok(offsets);
                }
            }
            self.lx.expect(POSVEROFFDIR)?;
            offsets.vertical = Some(self.lx.text()?);
        }
        Ok(offsets)
    }

    /// [§15.14.4] `<distance expression text representation>`.
    fn disexp(&mut self, depth: usize) -> Result<DistanceExp> {
        deeper(depth)?;
        self.lx.expect(DISEXP)?;
        let along = self.measure_representation()?;
        let mut names = ReferentNames::default();
        if self.lx.eat(FROM) {
            (names.from_feature, names.from_name) = self.referent_names()?;
        }
        if self.lx.eat(TOWARDS) {
            (names.towards_feature, names.towards_name) = self.referent_names()?;
        }
        let (mut lateral, mut vertical) = (None, None);
        if self.lx.eat_char(',') {
            if self.lx.eat(VECTOROFFSETS) {
                let vectors = self.vectors()?;
                return DistanceExp::new_vector(Some(along), names, Some(vectors));
            }
            if self.lx.peek_word().eq_ignore_ascii_case(LATERALOFFSET) {
                lateral = Some(self.lateral()?);
                if self.lx.eat_char(',') {
                    vertical = Some(self.vertical()?);
                }
            } else {
                vertical = Some(self.vertical()?);
            }
        }
        DistanceExp::new(Some(along), names, lateral, vertical)
    }

    /// [§15.14.4] `[<fid representation>] <referent name>`, after FROM or
    /// TOWARDS: of two texts, the first is the feature id.
    fn referent_names(&mut self) -> Result<(Option<String>, Option<String>)> {
        let first = self.lx.text()?;
        if self.lx.peek() == Some('"') {
            return Ok((Some(first), Some(self.lx.text()?)));
        }
        Ok((None, Some(first)))
    }

    /// [§15.14.4 Description 1h] `<lateral offset expression text
    /// representation>`.
    fn lateral(&mut self) -> Result<LatOffsetExp> {
        self.lx.expect(LATERALOFFSET)?;
        let (distance, geometry, description) = self.offset_text()?;
        Ok(LatOffsetExp { distance, geometry, description })
    }

    /// [§15.14.4 Description 1l] `<vertical offset expression text
    /// representation>`.
    fn vertical(&mut self) -> Result<VerOffsetExp> {
        self.lx.expect(VERTICALOFFSET)?;
        let (distance, geometry, description) = self.offset_text()?;
        Ok(VerOffsetExp { distance, geometry, description })
    }

    /// [§15.14.4 Description 1h, 1l] An offset's `<measure text>` (or
    /// \[EXTENSION\] NULL) and its optional referent text.
    fn offset_text(&mut self) -> Result<(Option<LrMeasure>, Option<Geom>, Option<String>)> {
        let distance = if self.lx.eat(NULL) { None } else { Some(self.measure_text()?) };
        if !self.lx.eat(FROM) {
            return Ok((distance, None, None));
        }
        if self.lx.peek() == Some('"') {
            return Ok((distance, None, Some(self.lx.text()?)));
        }
        Ok((distance, Some(read_geometry(self.lx.geometry_text()?)?), None))
    }

    /// [§15.14.4 Description 1n] `<vectors>`.
    fn vectors(&mut self) -> Result<VectorOffsetExp> {
        let vectors = self.list(Self::vector)?;
        if vectors.len() > MAX_VECTORS {
            return Err(self.lx.invalid("more than three vectors"));
        }
        VectorOffsetExp::new(vectors)
    }

    /// [§17.2.22] `<vector text representation>`.
    fn vector(&mut self) -> Result<Vector> {
        self.lx.expect(VECTOR)?;
        let z = self.lx.eat(Z);
        if self.lx.eat(EMPTY) {
            return Ok(Vector { x: None, y: None, z: None });
        }
        self.lx.expect_char('(')?;
        let (x, y) = (self.lx.number()?, self.lx.number()?);
        let third = if self.lx.peek() == Some(')') { None } else { Some(self.lx.number()?) };
        self.lx.expect_char(')')?;
        match (z, third) {
            (true, Some(zc)) => Ok(Vector::xyz(x, y, zc)),
            (false, None) => Ok(Vector::xy(x, y)),
            (true, None) | (false, Some(_)) => {
                Err(failed(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§17.2.22 Description 1d", "mixed coordinate dimensions"))
            }
        }
    }
}
