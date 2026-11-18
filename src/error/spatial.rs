//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §22, Table 16] The SQLSTATE
//! values of SQL/MM Spatial: its warnings in class 01 and its exceptions in
//! class 2F, SQL routine exception, each of a subclass F01 to F98.
//!
//! Full-Text [ISO/IEC 13249-2:2002 (CD, SC32 N0640)] gives 2FF01 to 2FF03
//! other meanings, and Spatial's 2FF02 and 2FF03 are its invalid argument
//! and null argument. Each part's conditions are raised only by its own
//! routines, so each value is kept as its part writes it, and the message
//! and the routine raising it tell the two apart (RULES.md
//! D132).

use super::SqlState;

/// [§22 Table 16] Each condition of SQL/MM Spatial: its SQLSTATE and its
/// subcondition, as the table writes it.
pub const SPATIAL_STATES: [(SqlState, &str); 98] = [
    (SqlState::SPATIAL_INVALID_POSITION, "invalid position"),
    (SqlState::SPATIAL_INVALID_ARGUMENT, "invalid argument"),
    (SqlState::SPATIAL_NULL_ARGUMENT, "null argument"),
    (SqlState::SPATIAL_INVALID_INTERSECTION_MATRIX, "invalid intersection matrix"),
    (SqlState::SPATIAL_DUPLICATE_VALUE, "duplicate value"),
    (SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "element is an empty set"),
    (SqlState::SPATIAL_NULL_EXTERIOR_RING, "null exterior ring"),
    (SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE, "element is not a valid type"),
    (SqlState::SPATIAL_ELEMENT_IS_A_NULL_VALUE, "element is a null value"),
    (SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "mixed spatial reference systems"),
    (SqlState::SPATIAL_NON_CONTIGUOUS_CURVES, "non-contiguous curves"),
    (SqlState::SPATIAL_CURVE_NOT_A_LINESTRING, "curve value is not a linestring value"),
    (SqlState::SPATIAL_ATTEMPTED_DIVISION_BY_ZERO, "attempted division by zero"),
    (SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED, "unsupported unit specified"),
    (SqlState::SPATIAL_FAILED_TO_TRANSFORM_GEOMETRY, "failed to transform geometry"),
    (SqlState::SPATIAL_NOT_AN_EMPTY_SET, "not an empty set"),
    (SqlState::SPATIAL_EMPTY_POINT_VALUE, "empty point value"),
    (SqlState::SPATIAL_POINT_VALUE_NOT_WELL_FORMED, "point value not well formed"),
    (SqlState::SPATIAL_POINTS_ARE_EQUAL, "points are equal"),
    (SqlState::SPATIAL_LINESTRING_IS_NOT_A_LINE, "linestring is not a line"),
    (SqlState::SPATIAL_DEGENERATE_LINE_HAS_NO_DIRECTION, "degenerate line has no direction"),
    (SqlState::SPATIAL_INVALID_WKT, "invalid well-known text representation"),
    (SqlState::SPATIAL_INVALID_WKB, "invalid well-known binary representation"),
    (SqlState::SPATIAL_INVALID_GML, "invalid GML representation"),
    (SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "mixed coordinate dimensions"),
    (SqlState::SPATIAL_DISCONNECTED_POINTS_NOT_INCLUDED, "disconnected points not included in result"),
    (SqlState::SPATIAL_COINCIDENT_EDGE, "coincident edge"),
    (SqlState::SPATIAL_COINCIDENT_NODE, "coincident node"),
    (SqlState::SPATIAL_CURVE_NOT_SIMPLE, "curve not simple"),
    (SqlState::SPATIAL_EDGE_CROSSES_NODE, "edge crosses node"),
    (SqlState::SPATIAL_EMPTY_NETWORK, "empty network"),
    (SqlState::SPATIAL_EMPTY_TOPOLOGY, "empty topology"),
    (SqlState::SPATIAL_END_NODE_NOT_GEOMETRY_END, "end node not geometry end point"),
    (SqlState::SPATIAL_GEOMETRY_CROSSES_A_NODE, "geometry crosses a node"),
    (SqlState::SPATIAL_GEOMETRY_CROSSES_AN_EDGE, "geometry crosses an edge"),
    (SqlState::SPATIAL_GEOMETRY_INTERSECTS_AN_EDGE, "geometry intersects an edge"),
    (SqlState::SPATIAL_GEOMETRY_NOT_WITHIN_FACE, "geometry not within face"),
    (SqlState::SPATIAL_LINK_HAS_NULL_GEOMETRY, "link has null geometry"),
    (SqlState::SPATIAL_NODES_IN_DIFFERENT_FACES, "nodes in different faces"),
    (SqlState::SPATIAL_NON_CONNECTED_EDGES, "non-connected edges"),
    (SqlState::SPATIAL_NON_CONNECTED_LINKS, "non-connected links"),
    (SqlState::SPATIAL_NON_EMPTY_VIEW, "non-empty view"),
    (SqlState::SPATIAL_NON_EXISTENT_EDGE, "non-existent edge"),
    (SqlState::SPATIAL_NON_EXISTENT_FACE, "non-existent face"),
    (SqlState::SPATIAL_NON_EXISTENT_LINK, "non-existent link"),
    (SqlState::SPATIAL_NON_EXISTENT_NODE, "non-existent node"),
    (SqlState::SPATIAL_NON_EXISTENT_SCHEMA, "non-existent schema"),
    (SqlState::SPATIAL_NON_EXISTENT_VIEW, "non-existent view"),
    (SqlState::SPATIAL_NOT_A_LOGICAL_LINK, "not a logical link"),
    (SqlState::SPATIAL_NOT_ISOLATED_NODE, "not isolated node"),
    (SqlState::SPATIAL_NOT_WITHIN_FACE, "not within face"),
    (SqlState::SPATIAL_OTHER_EDGES_CONNECTED, "other edges connected"),
    (SqlState::SPATIAL_OTHER_LINKS_CONNECTED, "other links connected"),
    (SqlState::SPATIAL_POINT_NOT_ON_EDGE, "point not on edge"),
    (SqlState::SPATIAL_POINT_NOT_ON_LINK, "point not on link"),
    (SqlState::SPATIAL_SCHEMA_ALREADY_EXISTS, "schema already exists"),
    (SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START, "start node not geometry start point"),
    (SqlState::SPATIAL_NULL_NODE_GEOMETRY, "null node geometry"),
    (SqlState::SPATIAL_UNKNOWN_SPATIAL_REFERENCE_SYSTEM, "unknown spatial reference system"),
    (SqlState::SPATIAL_UNIVERSAL_FACE_HAS_NO_GEOMETRY, "universal face has no geometry"),
    (SqlState::SPATIAL_INVALID_UNIVERSAL_FACE, "invalid universal face"),
    (SqlState::SPATIAL_INVALID_TOPOLOGY_NAME, "invalid topology name"),
    (SqlState::SPATIAL_TOPOLOGY_PRIVILEGE_DENIED, "topology privilege denied"),
    (SqlState::SPATIAL_INVALID_NETWORK_NAME, "invalid network name"),
    (SqlState::SPATIAL_NETWORK_PRIVILEGE_DENIED, "network privilege denied"),
    (SqlState::SPATIAL_TRIANGLES_CANNOT_HAVE_HOLES, "triangles cannot have holes"),
    (SqlState::SPATIAL_POLYGON_NOT_A_TRIANGLE, "polygon value is not a triangle value"),
    (SqlState::SPATIAL_NOT_A_TRIANGLE, "element is not an ST_Triangle type"),
    (SqlState::SPATIAL_NOT_A_POLYHDRLSURFACE, "element is not an ST_PolyhdrlSurface type"),
    (SqlState::SPATIAL_NOT_A_TIN, "element is not an ST_TIN type"),
    (SqlState::SPATIAL_AT_LEAST_THREE_POINTS, "at least 3 points are required"),
    (SqlState::SPATIAL_BOTH_GEOMETRIES_MUST_BE_3D, "both geometries must be 3D"),
    (SqlState::SPATIAL_GEOMETRY_IS_NOT_3D, "geometry is not 3D"),
    (SqlState::SPATIAL_INVALID_GEOMETRY, "invalid geometry"),
    (SqlState::SPATIAL_EXTERIOR_RING_NOT_FOUR_POINTS, "exterior ring must have exactly 4 points"),
    (SqlState::SPATIAL_CURVE_HAS_MULTIPLE_SEGMENTS, "curve has multiple segments"),
    (SqlState::SPATIAL_EXACTLY_THREE_POINTS, "exactly three points are required"),
    (SqlState::SPATIAL_POINTS_ARE_COLLINEAR, "points are collinear"),
    (SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE, "the given distance is longer than curve"),
    (SqlState::SPATIAL_POINT_NOT_ON_CURVE, "the point is not on the curve"),
    (SqlState::SPATIAL_INVALID_LRM, "invalid LRM"),
    (SqlState::SPATIAL_CHANGING_DEFAULT_MEASURE, "changing default measure may invalidate position expressions using this linear element"),
    (SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION, "potentially incompatible referent position and location"),
    (SqlState::SPATIAL_MISSING_MEASURE_VALUES, "missing measure value(s)"),
    (SqlState::SPATIAL_NON_CONTIGUOUS_SURFACES, "non-contiguous surfaces"),
    (SqlState::SPATIAL_INCORRECT_NUMBER_OF_VECTORS, "incorrect number of vectors"),
    (SqlState::SPATIAL_CANNOT_TRANSLATE, "cannot translate"),
    (SqlState::SPATIAL_INDETERMINATE_EQUALITY, "indeterminate equality"),
    (SqlState::SPATIAL_OFFSET_UNIT_MUST_BE_SPECIFIED, "offset unit must be specified"),
    (SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM, "towards referent requires a from referent"),
    (SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET, "illegal with vector offset"),
    (SqlState::SPATIAL_ILLEGAL_WITH_LATERAL_OFFSET, "illegal with lateral offset"),
    (SqlState::SPATIAL_ILLEGAL_WITH_VERTICAL_OFFSET, "illegal with vertical offset"),
    (SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_DESCRIPTION, "illegal with offset referent description"),
    (SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_GEOMETRY, "illegal with offset referent geometry"),
    (SqlState::SPATIAL_MIXED_IS3D, "mixed Is3D"),
    (SqlState::SPATIAL_M_COORDINATES_NOT_ALLOWED, "m coordinates not allowed"),
    (SqlState::SPATIAL_NULL_EXTERIOR_SHELL, "null exterior shell"),
];

impl SqlState {
    /// [§22 Table 16] SQL/MM Spatial warning: invalid position.
    pub const SPATIAL_INVALID_POSITION: SqlState = SqlState::of_bytes(*b"01F01");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid argument.
    pub const SPATIAL_INVALID_ARGUMENT: SqlState = SqlState::of_bytes(*b"2FF02");
    /// [§22 Table 16] SQL/MM Spatial exception: null argument.
    pub const SPATIAL_NULL_ARGUMENT: SqlState = SqlState::of_bytes(*b"2FF03");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid intersection matrix.
    pub const SPATIAL_INVALID_INTERSECTION_MATRIX: SqlState = SqlState::of_bytes(*b"2FF04");
    /// [§22 Table 16] SQL/MM Spatial exception: duplicate value.
    pub const SPATIAL_DUPLICATE_VALUE: SqlState = SqlState::of_bytes(*b"2FF05");
    /// [§22 Table 16] SQL/MM Spatial exception: element is an empty set.
    pub const SPATIAL_ELEMENT_IS_AN_EMPTY_SET: SqlState = SqlState::of_bytes(*b"2FF06");
    /// [§22 Table 16] SQL/MM Spatial exception: null exterior ring.
    pub const SPATIAL_NULL_EXTERIOR_RING: SqlState = SqlState::of_bytes(*b"2FF07");
    /// [§22 Table 16] SQL/MM Spatial exception: element is not a valid type.
    pub const SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE: SqlState = SqlState::of_bytes(*b"2FF08");
    /// [§22 Table 16] SQL/MM Spatial exception: element is a null value.
    pub const SPATIAL_ELEMENT_IS_A_NULL_VALUE: SqlState = SqlState::of_bytes(*b"2FF09");
    /// [§22 Table 16] SQL/MM Spatial exception: mixed spatial reference systems.
    pub const SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS: SqlState = SqlState::of_bytes(*b"2FF10");
    /// [§22 Table 16] SQL/MM Spatial exception: non-contiguous curves.
    pub const SPATIAL_NON_CONTIGUOUS_CURVES: SqlState = SqlState::of_bytes(*b"2FF11");
    /// [§22 Table 16] SQL/MM Spatial exception: curve value is not a linestring value.
    pub const SPATIAL_CURVE_NOT_A_LINESTRING: SqlState = SqlState::of_bytes(*b"2FF12");
    /// [§22 Table 16] SQL/MM Spatial exception: attempted division by zero.
    pub const SPATIAL_ATTEMPTED_DIVISION_BY_ZERO: SqlState = SqlState::of_bytes(*b"2FF13");
    /// [§22 Table 16] SQL/MM Spatial exception: unsupported unit specified.
    pub const SPATIAL_UNSUPPORTED_UNIT_SPECIFIED: SqlState = SqlState::of_bytes(*b"2FF14");
    /// [§22 Table 16] SQL/MM Spatial exception: failed to transform geometry.
    pub const SPATIAL_FAILED_TO_TRANSFORM_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF15");
    /// [§22 Table 16] SQL/MM Spatial exception: not an empty set.
    pub const SPATIAL_NOT_AN_EMPTY_SET: SqlState = SqlState::of_bytes(*b"2FF16");
    /// [§22 Table 16] SQL/MM Spatial exception: empty point value.
    pub const SPATIAL_EMPTY_POINT_VALUE: SqlState = SqlState::of_bytes(*b"2FF17");
    /// [§22 Table 16] SQL/MM Spatial exception: point value not well formed.
    pub const SPATIAL_POINT_VALUE_NOT_WELL_FORMED: SqlState = SqlState::of_bytes(*b"2FF18");
    /// [§22 Table 16] SQL/MM Spatial exception: points are equal.
    pub const SPATIAL_POINTS_ARE_EQUAL: SqlState = SqlState::of_bytes(*b"2FF19");
    /// [§22 Table 16] SQL/MM Spatial exception: linestring is not a line.
    pub const SPATIAL_LINESTRING_IS_NOT_A_LINE: SqlState = SqlState::of_bytes(*b"2FF20");
    /// [§22 Table 16] SQL/MM Spatial exception: degenerate line has no direction.
    pub const SPATIAL_DEGENERATE_LINE_HAS_NO_DIRECTION: SqlState = SqlState::of_bytes(*b"2FF21");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid well-known text representation.
    pub const SPATIAL_INVALID_WKT: SqlState = SqlState::of_bytes(*b"2FF22");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid well-known binary representation.
    pub const SPATIAL_INVALID_WKB: SqlState = SqlState::of_bytes(*b"2FF23");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid GML representation.
    pub const SPATIAL_INVALID_GML: SqlState = SqlState::of_bytes(*b"2FF24");
    /// [§22 Table 16] SQL/MM Spatial exception: mixed coordinate dimensions.
    pub const SPATIAL_MIXED_COORDINATE_DIMENSIONS: SqlState = SqlState::of_bytes(*b"2FF25");
    /// [§22 Table 16] SQL/MM Spatial warning: disconnected points not included in result.
    pub const SPATIAL_DISCONNECTED_POINTS_NOT_INCLUDED: SqlState = SqlState::of_bytes(*b"01F26");
    /// [§22 Table 16] SQL/MM Spatial exception: coincident edge.
    pub const SPATIAL_COINCIDENT_EDGE: SqlState = SqlState::of_bytes(*b"2FF27");
    /// [§22 Table 16] SQL/MM Spatial exception: coincident node.
    pub const SPATIAL_COINCIDENT_NODE: SqlState = SqlState::of_bytes(*b"2FF28");
    /// [§22 Table 16] SQL/MM Spatial exception: curve not simple.
    pub const SPATIAL_CURVE_NOT_SIMPLE: SqlState = SqlState::of_bytes(*b"2FF29");
    /// [§22 Table 16] SQL/MM Spatial exception: edge crosses node.
    pub const SPATIAL_EDGE_CROSSES_NODE: SqlState = SqlState::of_bytes(*b"2FF30");
    /// [§22 Table 16] SQL/MM Spatial exception: empty network.
    pub const SPATIAL_EMPTY_NETWORK: SqlState = SqlState::of_bytes(*b"2FF31");
    /// [§22 Table 16] SQL/MM Spatial exception: empty topology.
    pub const SPATIAL_EMPTY_TOPOLOGY: SqlState = SqlState::of_bytes(*b"2FF32");
    /// [§22 Table 16] SQL/MM Spatial exception: end node not geometry end point.
    pub const SPATIAL_END_NODE_NOT_GEOMETRY_END: SqlState = SqlState::of_bytes(*b"2FF33");
    /// [§22 Table 16] SQL/MM Spatial exception: geometry crosses a node.
    pub const SPATIAL_GEOMETRY_CROSSES_A_NODE: SqlState = SqlState::of_bytes(*b"2FF34");
    /// [§22 Table 16] SQL/MM Spatial exception: geometry crosses an edge.
    pub const SPATIAL_GEOMETRY_CROSSES_AN_EDGE: SqlState = SqlState::of_bytes(*b"2FF35");
    /// [§22 Table 16] SQL/MM Spatial exception: geometry intersects an edge.
    pub const SPATIAL_GEOMETRY_INTERSECTS_AN_EDGE: SqlState = SqlState::of_bytes(*b"2FF36");
    /// [§22 Table 16] SQL/MM Spatial exception: geometry not within face.
    pub const SPATIAL_GEOMETRY_NOT_WITHIN_FACE: SqlState = SqlState::of_bytes(*b"2FF37");
    /// [§22 Table 16] SQL/MM Spatial exception: link has null geometry.
    pub const SPATIAL_LINK_HAS_NULL_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF38");
    /// [§22 Table 16] SQL/MM Spatial exception: nodes in different faces.
    pub const SPATIAL_NODES_IN_DIFFERENT_FACES: SqlState = SqlState::of_bytes(*b"2FF39");
    /// [§22 Table 16] SQL/MM Spatial exception: non-connected edges.
    pub const SPATIAL_NON_CONNECTED_EDGES: SqlState = SqlState::of_bytes(*b"2FF40");
    /// [§22 Table 16] SQL/MM Spatial exception: non-connected links.
    pub const SPATIAL_NON_CONNECTED_LINKS: SqlState = SqlState::of_bytes(*b"2FF41");
    /// [§22 Table 16] SQL/MM Spatial exception: non-empty view.
    pub const SPATIAL_NON_EMPTY_VIEW: SqlState = SqlState::of_bytes(*b"2FF42");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent edge.
    pub const SPATIAL_NON_EXISTENT_EDGE: SqlState = SqlState::of_bytes(*b"2FF43");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent face.
    pub const SPATIAL_NON_EXISTENT_FACE: SqlState = SqlState::of_bytes(*b"2FF44");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent link.
    pub const SPATIAL_NON_EXISTENT_LINK: SqlState = SqlState::of_bytes(*b"2FF45");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent node.
    pub const SPATIAL_NON_EXISTENT_NODE: SqlState = SqlState::of_bytes(*b"2FF46");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent schema.
    pub const SPATIAL_NON_EXISTENT_SCHEMA: SqlState = SqlState::of_bytes(*b"2FF47");
    /// [§22 Table 16] SQL/MM Spatial exception: non-existent view.
    pub const SPATIAL_NON_EXISTENT_VIEW: SqlState = SqlState::of_bytes(*b"2FF48");
    /// [§22 Table 16] SQL/MM Spatial exception: not a logical link.
    pub const SPATIAL_NOT_A_LOGICAL_LINK: SqlState = SqlState::of_bytes(*b"2FF49");
    /// [§22 Table 16] SQL/MM Spatial exception: not isolated node.
    pub const SPATIAL_NOT_ISOLATED_NODE: SqlState = SqlState::of_bytes(*b"2FF50");
    /// [§22 Table 16] SQL/MM Spatial exception: not within face.
    pub const SPATIAL_NOT_WITHIN_FACE: SqlState = SqlState::of_bytes(*b"2FF51");
    /// [§22 Table 16] SQL/MM Spatial exception: other edges connected.
    pub const SPATIAL_OTHER_EDGES_CONNECTED: SqlState = SqlState::of_bytes(*b"2FF52");
    /// [§22 Table 16] SQL/MM Spatial exception: other links connected.
    pub const SPATIAL_OTHER_LINKS_CONNECTED: SqlState = SqlState::of_bytes(*b"2FF53");
    /// [§22 Table 16] SQL/MM Spatial exception: point not on edge.
    pub const SPATIAL_POINT_NOT_ON_EDGE: SqlState = SqlState::of_bytes(*b"2FF54");
    /// [§22 Table 16] SQL/MM Spatial exception: point not on link.
    pub const SPATIAL_POINT_NOT_ON_LINK: SqlState = SqlState::of_bytes(*b"2FF55");
    /// [§22 Table 16] SQL/MM Spatial exception: schema already exists.
    pub const SPATIAL_SCHEMA_ALREADY_EXISTS: SqlState = SqlState::of_bytes(*b"2FF56");
    /// [§22 Table 16] SQL/MM Spatial exception: start node not geometry start point.
    pub const SPATIAL_START_NODE_NOT_GEOMETRY_START: SqlState = SqlState::of_bytes(*b"2FF57");
    /// [§22 Table 16] SQL/MM Spatial exception: null node geometry.
    pub const SPATIAL_NULL_NODE_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF58");
    /// [§22 Table 16] SQL/MM Spatial exception: unknown spatial reference system.
    pub const SPATIAL_UNKNOWN_SPATIAL_REFERENCE_SYSTEM: SqlState = SqlState::of_bytes(*b"2FF59");
    /// [§22 Table 16] SQL/MM Spatial exception: universal face has no geometry.
    pub const SPATIAL_UNIVERSAL_FACE_HAS_NO_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF60");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid universal face.
    pub const SPATIAL_INVALID_UNIVERSAL_FACE: SqlState = SqlState::of_bytes(*b"2FF61");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid topology name.
    pub const SPATIAL_INVALID_TOPOLOGY_NAME: SqlState = SqlState::of_bytes(*b"2FF62");
    /// [§22 Table 16] SQL/MM Spatial exception: topology privilege denied.
    pub const SPATIAL_TOPOLOGY_PRIVILEGE_DENIED: SqlState = SqlState::of_bytes(*b"2FF63");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid network name.
    pub const SPATIAL_INVALID_NETWORK_NAME: SqlState = SqlState::of_bytes(*b"2FF64");
    /// [§22 Table 16] SQL/MM Spatial exception: network privilege denied.
    pub const SPATIAL_NETWORK_PRIVILEGE_DENIED: SqlState = SqlState::of_bytes(*b"2FF65");
    /// [§22 Table 16] SQL/MM Spatial exception: triangles cannot have holes.
    pub const SPATIAL_TRIANGLES_CANNOT_HAVE_HOLES: SqlState = SqlState::of_bytes(*b"2FF66");
    /// [§22 Table 16] SQL/MM Spatial exception: polygon value is not a triangle value.
    pub const SPATIAL_POLYGON_NOT_A_TRIANGLE: SqlState = SqlState::of_bytes(*b"2FF67");
    /// [§22 Table 16] SQL/MM Spatial exception: element is not an ST_Triangle type.
    pub const SPATIAL_NOT_A_TRIANGLE: SqlState = SqlState::of_bytes(*b"2FF68");
    /// [§22 Table 16] SQL/MM Spatial exception: element is not an ST_PolyhdrlSurface type.
    pub const SPATIAL_NOT_A_POLYHDRLSURFACE: SqlState = SqlState::of_bytes(*b"2FF69");
    /// [§22 Table 16] SQL/MM Spatial exception: element is not an ST_TIN type.
    pub const SPATIAL_NOT_A_TIN: SqlState = SqlState::of_bytes(*b"2FF70");
    /// [§22 Table 16] SQL/MM Spatial exception: at least 3 points are required.
    pub const SPATIAL_AT_LEAST_THREE_POINTS: SqlState = SqlState::of_bytes(*b"2FF71");
    /// [§22 Table 16] SQL/MM Spatial exception: both geometries must be 3D.
    pub const SPATIAL_BOTH_GEOMETRIES_MUST_BE_3D: SqlState = SqlState::of_bytes(*b"2FF72");
    /// [§22 Table 16] SQL/MM Spatial exception: geometry is not 3D.
    pub const SPATIAL_GEOMETRY_IS_NOT_3D: SqlState = SqlState::of_bytes(*b"2FF73");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid geometry.
    pub const SPATIAL_INVALID_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF74");
    /// [§22 Table 16] SQL/MM Spatial exception: exterior ring must have exactly 4 points.
    pub const SPATIAL_EXTERIOR_RING_NOT_FOUR_POINTS: SqlState = SqlState::of_bytes(*b"2FF75");
    /// [§22 Table 16] SQL/MM Spatial exception: curve has multiple segments.
    pub const SPATIAL_CURVE_HAS_MULTIPLE_SEGMENTS: SqlState = SqlState::of_bytes(*b"2FF76");
    /// [§22 Table 16] SQL/MM Spatial exception: exactly three points are required.
    pub const SPATIAL_EXACTLY_THREE_POINTS: SqlState = SqlState::of_bytes(*b"2FF77");
    /// [§22 Table 16] SQL/MM Spatial exception: points are collinear.
    pub const SPATIAL_POINTS_ARE_COLLINEAR: SqlState = SqlState::of_bytes(*b"2FF78");
    /// [§22 Table 16] SQL/MM Spatial exception: the given distance is longer than curve.
    pub const SPATIAL_DISTANCE_LONGER_THAN_CURVE: SqlState = SqlState::of_bytes(*b"2FF79");
    /// [§22 Table 16] SQL/MM Spatial exception: the point is not on the curve.
    pub const SPATIAL_POINT_NOT_ON_CURVE: SqlState = SqlState::of_bytes(*b"2FF80");
    /// [§22 Table 16] SQL/MM Spatial exception: invalid LRM.
    pub const SPATIAL_INVALID_LRM: SqlState = SqlState::of_bytes(*b"2FF81");
    /// [§22 Table 16] SQL/MM Spatial warning: changing default measure may invalidate position expressions using this linear element.
    pub const SPATIAL_CHANGING_DEFAULT_MEASURE: SqlState = SqlState::of_bytes(*b"01F82");
    /// [§22 Table 16] SQL/MM Spatial exception: potentially incompatible referent position and location.
    pub const SPATIAL_INCOMPATIBLE_REFERENT_POSITION: SqlState = SqlState::of_bytes(*b"2FF83");
    /// [§22 Table 16] SQL/MM Spatial exception: missing measure value(s).
    pub const SPATIAL_MISSING_MEASURE_VALUES: SqlState = SqlState::of_bytes(*b"2FF84");
    /// [§22 Table 16] SQL/MM Spatial exception: non-contiguous surfaces.
    pub const SPATIAL_NON_CONTIGUOUS_SURFACES: SqlState = SqlState::of_bytes(*b"2FF85");
    /// [§22 Table 16] SQL/MM Spatial exception: incorrect number of vectors.
    pub const SPATIAL_INCORRECT_NUMBER_OF_VECTORS: SqlState = SqlState::of_bytes(*b"2FF86");
    /// [§22 Table 16] SQL/MM Spatial exception: cannot translate.
    pub const SPATIAL_CANNOT_TRANSLATE: SqlState = SqlState::of_bytes(*b"2FF87");
    /// [§22 Table 16] SQL/MM Spatial exception: indeterminate equality.
    pub const SPATIAL_INDETERMINATE_EQUALITY: SqlState = SqlState::of_bytes(*b"2FF88");
    /// [§22 Table 16] SQL/MM Spatial exception: offset unit must be specified.
    pub const SPATIAL_OFFSET_UNIT_MUST_BE_SPECIFIED: SqlState = SqlState::of_bytes(*b"2FF89");
    /// [§22 Table 16] SQL/MM Spatial exception: towards referent requires a from referent.
    pub const SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM: SqlState = SqlState::of_bytes(*b"2FF90");
    /// [§22 Table 16] SQL/MM Spatial exception: illegal with vector offset.
    pub const SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET: SqlState = SqlState::of_bytes(*b"2FF91");
    /// [§22 Table 16] SQL/MM Spatial exception: illegal with lateral offset.
    pub const SPATIAL_ILLEGAL_WITH_LATERAL_OFFSET: SqlState = SqlState::of_bytes(*b"2FF92");
    /// [§22 Table 16] SQL/MM Spatial exception: illegal with vertical offset.
    pub const SPATIAL_ILLEGAL_WITH_VERTICAL_OFFSET: SqlState = SqlState::of_bytes(*b"2FF93");
    /// [§22 Table 16] SQL/MM Spatial exception: illegal with offset referent description.
    pub const SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_DESCRIPTION: SqlState = SqlState::of_bytes(*b"2FF94");
    /// [§22 Table 16] SQL/MM Spatial exception: illegal with offset referent geometry.
    pub const SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_GEOMETRY: SqlState = SqlState::of_bytes(*b"2FF95");
    /// [§22 Table 16] SQL/MM Spatial exception: mixed Is3D.
    pub const SPATIAL_MIXED_IS3D: SqlState = SqlState::of_bytes(*b"2FF96");
    /// [§22 Table 16] SQL/MM Spatial exception: m coordinates not allowed.
    pub const SPATIAL_M_COORDINATES_NOT_ALLOWED: SqlState = SqlState::of_bytes(*b"2FF97");
    /// [§22 Table 16] SQL/MM Spatial exception: null exterior shell.
    pub const SPATIAL_NULL_EXTERIOR_SHELL: SqlState = SqlState::of_bytes(*b"2FF98");
}
