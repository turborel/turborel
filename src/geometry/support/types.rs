//! The types through the SqlType contract (RULES.md D53,
//! D123), and the families of one type ST_SpatialRefSys and
//! ST_Vector are, so they are ordered EQUALS ONLY BY RELATIVE as the
//! geometry types are (`value/types/family.rs`).
//!
//! A value is shown, and CAST to a character string, as its well-known
//! text [§14.1.8, §16.1.20, §16.2.22, §17.2.21], the transform group
//! ST_WellKnownText; a type of no transform group is shown as the
//! constructor that makes the value. CAST from a character string reads
//! the well-known text; ST_Angle and ST_Direction CAST from and to a
//! number as their radians, and ST_Vector from and to a binary string as
//! its well-known binary, the groups ST_WellKnownBinary.

use std::cmp::Ordering as Order;

use super::values::{self, Knot, NurbsPoint, Placement, Vector, angle_of, angle_value, direction_of, direction_value, not_of};
use super::vector_formats::{read_binary, read_text, write_binary, write_text};
use super::{Ordering, SUPPORTS, Support, kind_of, of_value, ops, wrap};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::spatial::srs::angle::Angle;
use crate::spatial::srs::direction::Direction;
use crate::value::cast::uncastable;
use crate::value::lob::LobKind;
use crate::value::types::family::Family;
use crate::value::types::{Descriptor, Kind as TypeKind};
use crate::value::{Binary, Bucket, DataType, SqlType, Value};

/// One of the types, as a kind of type.
#[derive(Debug)]
pub struct SupportType {
    which: Support,
}

/// The types, as `SUPPORTS` orders them.
pub(super) static TYPES: [SupportType; 7] = [
    SupportType { which: SUPPORTS[0] },
    SupportType { which: SUPPORTS[1] },
    SupportType { which: SUPPORTS[2] },
    SupportType { which: SUPPORTS[3] },
    SupportType { which: SUPPORTS[4] },
    SupportType { which: SUPPORTS[5] },
    SupportType { which: SUPPORTS[6] },
];

/// The family of one type, ordered EQUALS ONLY.
#[derive(Debug)]
pub struct OneType(Support);

static SYSTEMS: OneType = OneType(Support::SpatialRefSys);
static VECTORS: OneType = OneType(Support::Vector);

/// The order-preserving bit of an `f64` key.
const SIGN: u64 = 1 << 63;

impl SupportType {
    /// Its type.
    pub fn which(&self) -> Support {
        self.which
    }

    /// The radians of an angle or a direction.
    fn radians(&self, v: &Value) -> Option<f64> {
        let (s, bytes) = of_value(v).filter(|(s, _)| *s == self.which)?;
        matches!(s, Support::Angle | Support::Direction).then(|| values::direction_radians(bytes)).flatten()
    }

    /// The bytes of `v`, a value of it.
    fn bytes<'v>(&self, v: &'v Value) -> Result<&'v [u8]> {
        of_value(v).filter(|(s, _)| *s == self.which).map(|(_, b)| b).ok_or_else(|| not_of(self.which))
    }

    /// The value of the well-known text `text`.
    fn of_text(&self, text: &str) -> Result<Value> {
        let f = |e: crate::spatial::srs::Fault| ops::fault(&e);
        match self.which {
            // [§14.1.8] as ST_WKTSRSToSQL, of the systems supplied.
            Support::SpatialRefSys => super::systems::from_text(text, None),
            Support::Angle => Ok(angle_value(Angle::from_text(text).map_err(f)?)),
            Support::Direction => Ok(direction_value(Direction::from_text(text).map_err(f)?)),
            Support::Vector => Ok(read_text(text, 0)?.value()),
            Support::AffinePlacement | Support::NurbsPoint | Support::Knot => {
                uncastable(&Value::Char(text.to_owned()), &super::data_type(self.which))
            }
        }
    }

    /// The value `v`, not of it, cast to it [§14.1.8, §16.1.20, §16.2.22,
    /// §17.2.21].
    fn converted(&self, v: &Value) -> Result<Value> {
        if let Value::Char(text) = v {
            return self.of_text(text.trim_end_matches(' '));
        }
        if let Value::Lob(lob) = v
            && lob.kind() == LobKind::Character
        {
            return self.of_text(&ops::text(std::slice::from_ref(v), 0)?);
        }
        let number = v.as_f64().filter(|_| matches!(self.which, Support::Angle | Support::Direction));
        if let Some(radians) = number {
            if self.which == Support::Angle {
                return Ok(angle_value(Angle::new(radians)));
            }
            return Direction::new(radians).map(direction_value).map_err(|e| ops::fault(&e));
        }
        let lob_kind = if let Value::Lob(lob) = v { Some(lob.kind()) } else { None };
        let binary = matches!(v, Value::Binary(_)) || lob_kind == Some(LobKind::Binary);
        if self.which == Support::Vector && binary {
            return Ok(read_binary(&ops::octets(std::slice::from_ref(v), 0)?, 0)?.value());
        }
        uncastable(v, &super::data_type(self.which))
    }
}

/// `text` quoted as a character string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The literal of a geometry payload.
fn geometry_literal(bytes: &[u8]) -> String {
    crate::geometry::GEOMETRY.literal(&crate::geometry::kind::wrap(bytes.to_vec()))
}

/// A value of attributes `parts`, each a method and its argument's
/// literal if the attribute is present: the constructor `name` of them
/// all if all are, else the constructor function and the mutators.
fn constructed(name: &str, parts: &[(&str, Option<String>)]) -> String {
    if let Some(all) = parts.iter().map(|(_, p)| p.clone()).collect::<Option<Vec<String>>>() {
        return format!("{name}({})", all.join(", "));
    }
    let set: String = parts.iter().filter_map(|(m, p)| p.as_ref().map(|p| format!(".{m}({p})"))).collect();
    if set.is_empty() { format!("NEW {name}()") } else { format!("(NEW {name}()){set}") }
}

/// How a value of a type of no transform group is written.
fn shown(s: Support, v: &Value) -> Result<String> {
    let float = |x: Option<f64>| x.map(|x| format!("{x:e}"));
    let integer = |n: Option<i32>| n.map(|n| n.to_string());
    Ok(match s {
        Support::Knot => {
            let k = Knot::of(v)?;
            constructed(s.name(), &[("ST_VALUE", float(k.value)), ("ST_MULTIPLICITY", integer(k.multiplicity))])
        }
        Support::NurbsPoint => {
            let n = NurbsPoint::of(v)?;
            constructed(s.name(), &[("ST_WEIGHTEDPOINT", n.point.as_deref().map(geometry_literal)), ("ST_WEIGHT", float(n.weight))])
        }
        Support::AffinePlacement => {
            let p = Placement::of(v)?;
            let vectors = |d: &Vec<Vector>| {
                let each: Vec<String> = d.iter().map(|each| TYPES[3].literal(&each.value())).collect();
                format!("ARRAY[{}]", each.join(", "))
            };
            let parts =
                [("ST_LOCATION", p.location.as_deref().map(geometry_literal)), ("ST_REFDIRECTIONS", p.directions.as_ref().map(vectors))];
            constructed(s.name(), &parts)
        }
        Support::SpatialRefSys | Support::Angle | Support::Direction | Support::Vector => text_of(s, v)?,
    })
}

/// The well-known text of a value of a type of a transform group.
fn text_of(s: Support, v: &Value) -> Result<String> {
    match s {
        Support::SpatialRefSys => Ok(values::system_of(v)?.as_wkt().unwrap_or_default()),
        Support::Angle => Ok(angle_of(v)?.as_text()),
        Support::Direction => Ok(direction_of(v)?.as_text()),
        Support::Vector => write_text(&Vector::of(v)?),
        Support::AffinePlacement | Support::NurbsPoint | Support::Knot => shown(s, v),
    }
}

/// SQLCODE -900: no key is of a type ordered EQUALS ONLY or of none.
fn unkeyed(t: &DataType) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] no key is of {t}, whose values are not ordered (RULES.md D81, D123)"))
}

impl SqlType for SupportType {
    fn name(&self) -> &str {
        self.which.name()
    }

    fn family(&self) -> Option<&'static dyn Family> {
        match self.which {
            Support::SpatialRefSys => Some(&SYSTEMS),
            Support::Vector => Some(&VECTORS),
            Support::Angle | Support::Direction | Support::AffinePlacement | Support::NurbsPoint | Support::Knot => None,
        }
    }

    fn written(&self, _: &DataType) -> String {
        self.which.name().to_owned()
    }

    fn comparable(&self, other: &DataType) -> bool {
        super::of_data_type(other) == Some(self.which)
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Order> {
        if let (Some(x), Some(y)) = (self.radians(a), self.radians(b)) {
            return Some(x.total_cmp(&y));
        }
        Some(self.bytes(a).ok()?.cmp(self.bytes(b).ok()?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(self.bytes(v).ok()?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, _: &str) -> Result<Value> {
        self.bytes(&v)?;
        Ok(v)
    }

    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        if v.is_null() || self.bytes(&v).is_ok() {
            return Ok(v);
        }
        self.converted(&v)
    }

    fn lowered(&self, v: &Value, to: &DataType) -> Option<Value> {
        if to.is_character() {
            return text_of(self.which, v).ok().map(Value::Char);
        }
        if to.is_numeric() {
            return self.radians(v).map(Value::Approx);
        }
        if to.is_binary() && self.which == Support::Vector {
            return Vector::of(v).and_then(|v| write_binary(&v)).ok().map(|b| Value::Binary(Binary::new(b)));
        }
        None
    }

    fn literal(&self, v: &Value) -> String {
        let text = |t: Result<String>| t.map(|t| quoted(&t)).unwrap_or_default();
        match self.which {
            Support::SpatialRefSys => {
                let definition = text_of(self.which, v).unwrap_or_default();
                if definition.is_empty() { "ST_SPATIALREFSYS(0)".to_owned() } else { format!("ST_SPATIALREFSYS({})", quoted(&definition)) }
            }
            Support::Angle => format!("ST_ANGLEFROMTEXT({})", text(text_of(self.which, v))),
            Support::Direction => format!("ST_DIRECTIONFRMTXT({})", text(text_of(self.which, v))),
            Support::Vector => {
                let srid = Vector::of(v).map_or(0, |v| v.srid);
                format!("ST_VECTORFROMTEXT({}, {srid})", text(text_of(self.which, v)))
            }
            Support::AffinePlacement | Support::NurbsPoint | Support::Knot => shown(self.which, v).unwrap_or_default(),
        }
    }

    fn text(&self, v: &Value) -> String {
        text_of(self.which, v).unwrap_or_default()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        self.radians(v)
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(self.which.name())
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(self.bytes(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        values::check(self.which, raw).map_err(|_| r.corrupt("an invalid value of a spatial type"))?;
        Ok(wrap(self.which, raw.to_vec()))
    }

    fn ordered(&self) -> bool {
        self.which.ordering() != Ordering::None
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(super::data_type(self.which))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        Ok(self.bytes(v)?.to_vec())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        values::check(self.which, field).ok().map(|()| wrap(self.which, field.to_vec()))
    }

    /// The radians of an angle or a direction, as a DOUBLE PRECISION's
    /// key; no other is keyed (RULES.md D81, D123).
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let radians = self.radians(v).ok_or_else(|| unkeyed(t))?;
        let bits = (radians + 0.0_f64).to_bits();
        let ordered = if bits & SIGN == 0 { bits | SIGN } else { !bits };
        out.extend_from_slice(&ordered.to_be_bytes());
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        if self.which.ordering() != Ordering::Full {
            return Err(unkeyed(t));
        }
        let (word, after) = rest.split_first_chunk::<8>().ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] a key ends too soon"))?;
        *rest = after;
        let ordered = u64::from_be_bytes(*word);
        let radians = f64::from_bits(if ordered & SIGN == 0 { !ordered } else { ordered ^ SIGN });
        let mut out = Vec::new();
        out.put_u64(radians.to_bits());
        Ok(wrap(self.which, out))
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        self.bytes(v).ok().map(|_| (v.clone(), true))
    }
}

impl Family for OneType {
    fn is_subtype(&self, sub: &dyn SqlType, sup: &dyn SqlType) -> bool {
        super::support_kind(sub) == Some(self.0) && super::support_kind(sup) == Some(self.0)
    }

    fn named(&self, name: &str) -> Option<TypeKind> {
        (name == self.0.name()).then(|| kind_of(self.0))
    }

    fn most_specific(&self, v: &Value) -> Option<TypeKind> {
        of_value(v).filter(|(s, _)| *s == self.0).map(|_| kind_of(self.0))
    }

    fn supertype(&self, _: &dyn SqlType) -> Option<TypeKind> {
        None
    }

    fn instantiable(&self, _: &dyn SqlType) -> bool {
        true
    }

    fn relative_ordering(&self) -> &'static str {
        crate::geometry::ORDERING_EQUALS
    }

    fn kinds(&self) -> &'static [TypeKind] {
        let (all, at) = (super::kinds(), self.0.at());
        all.get(at..=at).unwrap_or(all)
    }
}
