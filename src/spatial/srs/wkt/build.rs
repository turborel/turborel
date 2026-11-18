//! Checks a tree of items against the productions of ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §14.1.9 and builds the coordinate system it holds.
//! Items appear in the order the productions give. The \[EXTENSION\] items
//! come last in their parent: TOWGS84 after a \<spheroid\>, AXIS after a
//! system's units, AUTHORITY at the end of any item.

use std::iter::Peekable;
use std::vec::IntoIter;

use super::super::fault::{Fault, Outcome};
use super::super::model::{
    Authority, Axis, CoordinateSystem, Datum, Geocentric, Geographic, Helmert, Parameter, PrimeMeridian, Projected, Projection, Spheroid,
    Unit,
};
use super::super::number;
use super::{
    AUTHORITY, AXIS, Arg, DATUM, DIRECTIONS, GEOCCS, GEOGCS, Node, PARAMETER, PRIMEM, PROJCS, PROJECTION, SPHEROID, TOWGS84, UNIT,
};

/// TOWGS84 holds three translations, or those and three rotations and a
/// scale difference.
const SHIFT_ONLY: usize = 3;
const SEVEN: usize = 7;
/// The AXIS items of a two- or three-dimensional system.
const PLANE: usize = 2;
const SPACE: usize = 3;

/// The <spatial reference system> `root` is.
pub(super) fn system(root: Node) -> Outcome<CoordinateSystem> {
    match root.keyword.as_str() {
        PROJCS => Ok(CoordinateSystem::Projected(Box::new(projected(root)?))),
        GEOGCS => Ok(CoordinateSystem::Geographic(Box::new(geographic(root)?))),
        GEOCCS => Ok(CoordinateSystem::Geocentric(Box::new(geocentric(root)?))),
        other => Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] {other} is not PROJCS, GEOGCS or GEOCCS"))),
    }
}

/// `<projected cs>`.
fn projected(node: Node) -> Outcome<Projected> {
    let mut args = Args::new(node);
    let name = args.text("<name>")?;
    let geographic = geographic(args.node(GEOGCS)?)?;
    let projection = projection(args.node(PROJECTION)?)?;
    let mut parameters = Vec::new();
    while let Some(item) = args.optional(PARAMETER) {
        parameters.push(parameter(item)?);
    }
    let linear_unit = unit(args.node(UNIT)?)?;
    let axes = args.axes(&[0, PLANE])?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(Projected { name, geographic, projection, parameters, linear_unit, axes, authority })
}

/// `<geographic cs>`. The <angular unit> and optional <linear unit> are
/// read as separated by a <comma>, which the production omits.
fn geographic(node: Node) -> Outcome<Geographic> {
    let mut args = Args::new(node);
    let name = args.text("<name>")?;
    let datum = datum(args.node(DATUM)?)?;
    let prime_meridian = prime_meridian(args.node(PRIMEM)?)?;
    let angular_unit = unit(args.node(UNIT)?)?;
    let linear_unit = args.optional(UNIT).map(unit).transpose()?;
    let axes = args.axes(&[0, PLANE, SPACE])?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(Geographic { name, datum, prime_meridian, angular_unit, linear_unit, axes, authority })
}

/// `<geocentric cs>`.
fn geocentric(node: Node) -> Outcome<Geocentric> {
    let mut args = Args::new(node);
    let name = args.text("<name>")?;
    let datum = datum(args.node(DATUM)?)?;
    let prime_meridian = prime_meridian(args.node(PRIMEM)?)?;
    let linear_unit = unit(args.node(UNIT)?)?;
    let axes = args.axes(&[0, SPACE])?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(Geocentric { name, datum, prime_meridian, linear_unit, axes, authority })
}

/// `<datum>`, then TOWGS84.
fn datum(node: Node) -> Outcome<Datum> {
    let mut args = Args::new(node);
    let name = args.text("<datum name>")?;
    let spheroid = spheroid(args.node(SPHEROID)?)?;
    let to_wgs84 = args.optional(TOWGS84).map(helmert).transpose()?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(Datum { name, spheroid, to_wgs84, authority })
}

/// `<spheroid>`: the semi-major axis is above zero (§14.1.9 Description
/// 1l); the inverse flattening is zero for a sphere or above one.
fn spheroid(node: Node) -> Outcome<Spheroid> {
    let mut args = Args::new(node);
    let name = args.text("<spheroid name>")?;
    let semi_major = args.number("<semi-major axis>")?;
    let inverse_flattening = args.number("<inverse flattening>")?;
    let authority = args.authority()?;
    args.finish()?;
    if semi_major <= 0.0_f64 {
        return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9 Description 1l] the <semi-major axis> is not above zero"));
    }
    if inverse_flattening < 0.0_f64 || (inverse_flattening > 0.0_f64 && inverse_flattening <= 1.0_f64) {
        return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9 Description 1k] the <inverse flattening> is neither zero nor above one"));
    }
    Ok(Spheroid { name, semi_major, inverse_flattening, authority })
}

/// `<prime meridian>`.
fn prime_meridian(node: Node) -> Outcome<PrimeMeridian> {
    let mut args = Args::new(node);
    let name = args.text("<prime meridian name>")?;
    let longitude = args.number("<longitude>")?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(PrimeMeridian { name, longitude, authority })
}

/// `<unit>`: the conversion factor is above zero (§14.1.9 Description 1m).
fn unit(node: Node) -> Outcome<Unit> {
    let mut args = Args::new(node);
    let name = args.text("<unit name>")?;
    let factor = args.number("<conversion factor>")?;
    let authority = args.authority()?;
    args.finish()?;
    if factor <= 0.0_f64 {
        return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9 Description 1m] the <conversion factor> is not above zero"));
    }
    Ok(Unit { name, factor, authority })
}

/// `<projection>`.
fn projection(node: Node) -> Outcome<Projection> {
    let mut args = Args::new(node);
    let name = args.text("<projection name>")?;
    let authority = args.authority()?;
    args.finish()?;
    Ok(Projection { name, authority })
}

/// `<parameter>`.
fn parameter(node: Node) -> Outcome<Parameter> {
    let mut args = Args::new(node);
    let name = args.text("<parameter name>")?;
    let value = args.number("<value>")?;
    args.finish()?;
    Ok(Parameter { name, value })
}

/// TOWGS84 with three or seven numbers.
fn helmert(node: Node) -> Outcome<Helmert> {
    let mut args = Args::new(node);
    let mut values = [0.0_f64; SEVEN];
    let mut count = 0_usize;
    for value in &mut values {
        if args.items.peek().is_none() {
            break;
        }
        *value = args.number("a TOWGS84 parameter")?;
        count = count.saturating_add(1);
    }
    args.finish()?;
    if count != SHIFT_ONLY && count != SEVEN {
        return Err(Fault::text("[EXTENSION] TOWGS84 takes three or seven numbers"));
    }
    let [tx, ty, tz, rx, ry, rz, scale] = values;
    Ok(Helmert { translation: [tx, ty, tz], rotation: [rx, ry, rz], scale })
}

/// AUTHORITY: an organization and its code, quoted.
fn authority(node: Node) -> Outcome<Authority> {
    let mut args = Args::new(node);
    let name = args.text("an authority name")?;
    let code = args.code()?;
    args.finish()?;
    Ok(Authority { name, code })
}

/// AXIS: a name and a direction key word.
fn axis(node: Node) -> Outcome<Axis> {
    let mut args = Args::new(node);
    let name = args.text("an axis name")?;
    let word = match args.items.next() {
        Some(Arg::Word(word)) => word,
        Some(Arg::Text(_) | Arg::Number(_) | Arg::Node(_)) | None => return Err(Fault::text("[EXTENSION] AXIS lacks its direction")),
    };
    args.finish()?;
    let direction = DIRECTIONS
        .iter()
        .find(|(key, _)| *key == word)
        .map(|(_, direction)| *direction)
        .ok_or_else(|| Fault::text(format!("[EXTENSION] {word} is not an axis direction")))?;
    Ok(Axis { name, direction })
}

/// Whether `arg` is an item with key word `keyword`.
fn is_item(arg: &Arg, keyword: &str) -> bool {
    match arg {
        Arg::Node(node) => node.keyword == keyword,
        Arg::Text(_) | Arg::Number(_) | Arg::Word(_) => false,
    }
}

/// The arguments of one item, read in order.
struct Args {
    keyword: String,
    items: Peekable<IntoIter<Arg>>,
}

impl Args {
    fn new(node: Node) -> Args {
        Args { keyword: node.keyword, items: node.args.into_iter().peekable() }
    }

    fn missing(&self, what: &str) -> Fault {
        Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] {} lacks {what}", self.keyword))
    }

    fn text(&mut self, what: &str) -> Outcome<String> {
        match self.items.next() {
            Some(Arg::Text(text)) => Ok(text),
            Some(Arg::Number(_) | Arg::Word(_) | Arg::Node(_)) | None => Err(self.missing(what)),
        }
    }

    fn number(&mut self, what: &str) -> Outcome<f64> {
        match self.items.next() {
            Some(Arg::Number(value)) => Ok(value),
            Some(Arg::Text(_) | Arg::Word(_) | Arg::Node(_)) | None => Err(self.missing(what)),
        }
    }

    /// An authority's code, quoted or a number.
    fn code(&mut self) -> Outcome<String> {
        match self.items.next() {
            Some(Arg::Text(text)) => Ok(text),
            Some(Arg::Number(value)) => Ok(number::shortest(value)),
            Some(Arg::Word(_) | Arg::Node(_)) | None => Err(self.missing("an authority code")),
        }
    }

    fn node(&mut self, keyword: &str) -> Outcome<Node> {
        self.optional(keyword).ok_or_else(|| self.missing(keyword))
    }

    fn optional(&mut self, keyword: &str) -> Option<Node> {
        if !self.items.peek().is_some_and(|arg| is_item(arg, keyword)) {
            return None;
        }
        match self.items.next() {
            Some(Arg::Node(node)) => Some(node),
            Some(Arg::Text(_) | Arg::Number(_) | Arg::Word(_)) | None => None,
        }
    }

    fn authority(&mut self) -> Outcome<Option<Authority>> {
        self.optional(AUTHORITY).map(authority).transpose()
    }

    /// The AXIS items, whose number is one of `counts`.
    fn axes(&mut self, counts: &[usize]) -> Outcome<Vec<Axis>> {
        let mut axes = Vec::new();
        while let Some(item) = self.optional(AXIS) {
            axes.push(axis(item)?);
        }
        if !counts.contains(&axes.len()) {
            return Err(Fault::text(format!("[EXTENSION] {} has {} AXIS items", self.keyword, axes.len())));
        }
        Ok(axes)
    }

    fn finish(mut self) -> Outcome<()> {
        if self.items.next().is_some() {
            return Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] {} has an unexpected argument", self.keyword)));
        }
        Ok(())
    }
}
