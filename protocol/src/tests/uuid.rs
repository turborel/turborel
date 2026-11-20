//! UUID values and columns, carried under their own tag.

use super::{MAX_BODY, Reply, unframed};
use crate::Column;
use turborel::value::types::uuid::{UuidType, octets_of, uuid_value};

/// Verifies: [EXTENSION] RULES.md D184: a UUID column's type is the tag
/// 27 alone, and a UUID value the tag then its 16 octets, counted as its
/// kind writes them; both read back as UUID with no type registry, as a
/// client reads them, and encode again the same.
#[test]
fn uuids_travel_under_their_own_tag() {
    let octets: [u8; 16] = std::array::from_fn(|i| u8::try_from(i * 17).unwrap());
    let column = Column { name: Some("ID".into()), data_type: UuidType::data_type(), not_null: true, declared: None };
    for reply in [Reply::Rows(vec![vec![uuid_value(octets)]]), Reply::Description(vec![column])] {
        let mut encoded = Vec::new();
        reply.encode(&mut encoded);
        let (tag, body) = unframed(&encoded, MAX_BODY).unwrap();
        let decoded = Reply::decode(tag, body).unwrap();
        if let Reply::Rows(rows) = &decoded {
            assert_eq!(octets_of(&rows[0][0]), Some(octets));
            let sent = [&[27_u8, 16, 0, 0, 0][..], &octets[..]].concat();
            assert!(body.windows(sent.len()).any(|w| w == sent), "the tag, the count and the octets");
        } else if let Reply::Description(columns) = &decoded {
            assert_eq!(columns[0].data_type, UuidType::data_type());
            assert!(body.contains(&27), "the tag");
        } else {
            panic!("unexpected {decoded:?}");
        }
        let mut resent = Vec::new();
        decoded.encode(&mut resent);
        assert_eq!(resent, encoded);
    }
}

/// Verifies: [EXTENSION] RULES.md D184: a UUID value whose octets are
/// not 16 is refused as malformed, not read as a UUID.
#[test]
fn a_short_uuid_is_refused() {
    let mut encoded = Vec::new();
    Reply::Rows(vec![vec![uuid_value([7; 16])]]).encode(&mut encoded);
    let (tag, body) = unframed(&encoded, MAX_BODY).unwrap();
    let at = body.windows(5).position(|w| w == [27, 16, 0, 0, 0]).unwrap();
    let mut short = body.to_vec();
    short[at + 1] = 15;
    short.remove(at + 5);
    assert!(Reply::decode(tag, &short).is_err());
}
