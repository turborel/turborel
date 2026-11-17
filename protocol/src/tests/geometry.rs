//! Geometry values carried by a client without a geometry implementation.

use super::{MAX_BODY, Reply, unframed};
use turborel::bytes::Reader;
use turborel::geometry::{GEOMETRY, Geometry};
use turborel::value::SqlType;
use turborel::value::types::parts;

/// Verifies: [DESIGN] D45, D53, D82: an unaware client preserves a
/// geometry's opaque bytes and SRID while displaying WKT; its re-encoded
/// reply is identical and the geometry type can decode the payload.
#[test]
fn opaque_geometry_round_trips_without_a_protocol_change() {
    for text in ["POINT (4.9 52.37)", "POINT EMPTY", "POLYGON ((0 0, 4 0, 4 4, 0 0))"] {
        let original = Geometry::from_wkt(text, 4326).unwrap();
        let mut encoded = Vec::new();
        Reply::Rows(vec![vec![original.to_value()]]).encode(&mut encoded);
        let (tag, body) = unframed(&encoded, MAX_BODY).unwrap();
        let decoded = Reply::decode(tag, body).unwrap();
        let Reply::Rows(rows) = &decoded else { panic!("expected rows") };
        let (name, displayed, payload) = parts(&rows[0][0]).unwrap();
        assert_eq!(name, "GEOMETRY");
        assert_eq!(displayed, text);
        let value = GEOMETRY.value(&mut Reader::received(&payload, "geometry")).unwrap();
        let restored = Geometry::from_value(&value).unwrap();
        assert_eq!(restored.srid(), 4_326_i32);
        assert_eq!(restored.to_wkt(), text);
        let mut resent = Vec::new();
        decoded.encode(&mut resent);
        assert_eq!(resent, encoded);
    }
}
