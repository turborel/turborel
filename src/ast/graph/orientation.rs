//! \[EXTENSION\] [P1 §4.1, Fig. 5] The seven orientations of an edge
//! pattern: each one's full form, its abbreviation, and the edges it
//! matches, as one table.

/// [P1 Fig. 5] Which edges an edge pattern matches, and which way it
/// walks them from the node before it to the node after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// `<-[ ]-`, `<-`: a directed edge, pointing left.
    Left,
    /// `~[ ]~`, `~`: an undirected edge.
    Undirected,
    /// `-[ ]->`, `->`: a directed edge, pointing right.
    Right,
    /// `<~[ ]~`, `<~`: pointing left, or undirected.
    LeftOrUndirected,
    /// `~[ ]~>`, `~>`: undirected, or pointing right.
    UndirectedOrRight,
    /// `<-[ ]->`, `<->`: pointing left or right.
    LeftOrRight,
    /// `-[ ]-`, `-`: pointing left, undirected or pointing right.
    Any,
}

/// The ways an edge may lie for an edge pattern to match it, walked from
/// the node pattern before the edge pattern to the one after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directions {
    /// A directed edge whose source is the node after.
    pub left: bool,
    /// An undirected edge.
    pub undirected: bool,
    /// A directed edge whose source is the node before.
    pub right: bool,
}

/// One row of [P1 Fig. 5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrientationForm {
    /// The orientation.
    pub orientation: Orientation,
    /// What opens its full form, before the filler's brackets' contents.
    pub opening: &'static str,
    /// What closes its full form, after them.
    pub closing: &'static str,
    /// Its abbreviation, which has no filler.
    pub abbreviation: &'static str,
    /// The edges it matches.
    pub directions: Directions,
}

/// [P1 Fig. 5] The table of edge patterns, in the figure's order.
pub const ORIENTATIONS: [OrientationForm; 7] = [
    row(Orientation::Left, ("<-[", "]-", "<-"), (true, false, false)),
    row(Orientation::Undirected, ("~[", "]~", "~"), (false, true, false)),
    row(Orientation::Right, ("-[", "]->", "->"), (false, false, true)),
    row(Orientation::LeftOrUndirected, ("<~[", "]~", "<~"), (true, true, false)),
    row(Orientation::UndirectedOrRight, ("~[", "]~>", "~>"), (false, true, true)),
    row(Orientation::LeftOrRight, ("<-[", "]->", "<->"), (true, false, true)),
    row(Orientation::Any, ("-[", "]-", "-"), (true, true, true)),
];

/// A row of the table: the orientation, its forms (opening, closing,
/// abbreviation), and whether it matches (left, undirected, right).
const fn row(orientation: Orientation, forms: (&'static str, &'static str, &'static str), ways: (bool, bool, bool)) -> OrientationForm {
    OrientationForm {
        orientation,
        opening: forms.0,
        closing: forms.1,
        abbreviation: forms.2,
        directions: Directions { left: ways.0, undirected: ways.1, right: ways.2 },
    }
}

impl Orientation {
    /// Its row of the table.
    #[must_use]
    pub fn form(self) -> OrientationForm {
        match self {
            Orientation::Left => ORIENTATIONS[0],
            Orientation::Undirected => ORIENTATIONS[1],
            Orientation::Right => ORIENTATIONS[2],
            Orientation::LeftOrUndirected => ORIENTATIONS[3],
            Orientation::UndirectedOrRight => ORIENTATIONS[4],
            Orientation::LeftOrRight => ORIENTATIONS[5],
            Orientation::Any => ORIENTATIONS[6],
        }
    }

    /// The edges it matches.
    #[must_use]
    pub fn directions(self) -> Directions {
        self.form().directions
    }

    /// The orientation matching what it does walked the other way: of a
    /// pattern read from its end, as a search may.
    #[must_use]
    pub fn reversed(self) -> Orientation {
        match self {
            Orientation::Left => Orientation::Right,
            Orientation::Right => Orientation::Left,
            Orientation::LeftOrUndirected => Orientation::UndirectedOrRight,
            Orientation::UndirectedOrRight => Orientation::LeftOrUndirected,
            Orientation::Undirected | Orientation::LeftOrRight | Orientation::Any => self,
        }
    }

    /// The orientation of these directions, if one matches just them.
    #[must_use]
    pub fn of(directions: Directions) -> Option<Orientation> {
        ORIENTATIONS.iter().find(|f| f.directions == directions).map(|f| f.orientation)
    }
}
