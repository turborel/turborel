//! The curve and surface engines of SQL/MM Spatial, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §7 and §8: lengths, end points, closedness, points
//! at a distance and linearization for every curve type, and exact
//! areas, perimeters, centroids and interior points for curve polygons,
//! polyhedral surfaces and TINs. They work on plain coordinates, so the
//! geometry model adapts onto them.

pub mod adapt;
pub mod arc;
pub mod cdt;
pub mod circular;
pub mod fault;
pub mod fresnel;
pub mod geodesy;
pub mod interior;
pub mod linearize;
pub mod numeric;
pub mod nurbs;
pub mod observe;
pub mod patches;
pub mod path;
pub mod piece;
pub mod simple;
pub mod spiral;
pub mod surface;
pub mod tin;
pub mod types;
