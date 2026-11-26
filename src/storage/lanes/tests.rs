use super::*;

/// A lane of `cells`, none a null.
fn lane(cells: &[Option<&str>]) -> Lane {
    let mut lane = Lane { nulls: vec![0; cells.len().div_ceil(8)], ..Lane::default() };
    for (n, cell) in cells.iter().enumerate() {
        match cell {
            Some(text) => lane.bytes.extend_from_slice(text.as_bytes()),
            None => set_null(&mut lane.nulls, n),
        }
        lane.ends.push(u32::try_from(lane.bytes.len()).unwrap());
    }
    lane
}

/// Verifies: [DESIGN] RULES.md D1148: a copy's columns read back as the
/// rows of text they hold, a null as a null; columns of another number of
/// cells, or a cell out of range or not UTF-8, are refused.
#[test]
fn lanes_read_back_as_rows_of_text() {
    let lanes = Lanes { rows: 3, columns: vec![lane(&[Some("1"), None, Some("33")]), lane(&[Some("a"), Some(""), Some("é")])] };
    let rows = lanes.to_rows().unwrap();
    let text = |s: &str| Value::Char(CharString::from(s.to_owned()));
    assert_eq!(
        format!("{rows:?}"),
        format!("{:?}", vec![vec![text("1"), text("a")], vec![Value::Null, text("")], vec![text("33"), text("é")]])
    );
    let short = Lanes { rows: 4, columns: lanes.columns.clone() };
    assert!(short.to_rows().is_err());
    let mut wild = lanes.columns[0].clone();
    wild.ends[2] = 99;
    assert!(wild.cell(2).is_err());
    let mut broken = lanes.columns[1].clone();
    broken.bytes[2] = 0xff;
    assert!(broken.cell(2).is_err());
}

/// Verifies: [DESIGN] RULES.md D1148: lanes appended and split at any row
/// hold the same rows, nulls and texts, in order.
#[test]
fn lanes_append_and_split_keep_their_rows() {
    let cells: Vec<Option<String>> = (0_usize..21).map(|n| (n % 5 != 3).then(|| format!("v{n}"))).collect();
    let as_lanes = |from: usize, to: usize| {
        let part: Vec<Option<&str>> = cells[from..to].iter().map(|c| c.as_deref()).collect();
        Lanes { rows: to - from, columns: vec![lane(&part), lane(&part)] }
    };
    let whole = as_lanes(0, 21).to_rows().unwrap();
    let mut joined = Lanes::default();
    for (from, to) in [(0, 3), (3, 11), (11, 12), (12, 21)] {
        joined.append(&as_lanes(from, to)).unwrap();
    }
    assert_eq!(format!("{:?}", joined.to_rows().unwrap()), format!("{whole:?}"));
    for at in [0, 1, 7, 8, 9, 20, 21] {
        let mut rest = joined.clone();
        let front = rest.split_front(at);
        assert_eq!((front.rows, rest.rows), (at, 21 - at));
        let mut again = front.to_rows().unwrap();
        again.extend(rest.to_rows().unwrap());
        assert_eq!(format!("{again:?}"), format!("{whole:?}"));
    }
    assert!(joined.append(&Lanes { rows: 1, columns: vec![lane(&[Some("x")])] }).is_err());
}
