//! Unit tests of a row's update written as a patch of its newest version:
//! what it leaves in the table is what the same row written whole leaves
//! (RULES.md R-TEST-1, R-TEST-2).

use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::version::Version;
use crate::engine::rows::{RowTable, TableFiles, TableOptions, key};
use crate::error::Code;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::index::Probe;
use crate::log::MemoryLog;
use crate::storage::{AfterImage, Condition, Conjunction, Patch, Row, RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// A number the zones keep, a text of varying width, a CHARACTER string
/// and a number more.
const TYPES: [DataType; 4] = [
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
    DataType::varying(100),
    DataType::character(8),
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
];

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn row(n: i128) -> Row {
    vec![int(n), Value::Char(format!("text {n}")), Value::Char(format!("{:<8}", "c")), int(n.saturating_mul(10))]
}

/// A table of `TYPES` in memory, with rows 1 to 3, its zones kept.
fn table(options: TableOptions) -> (RowTable, Vec<RowId>) {
    let fs = MemoryFileSystem::new();
    let pager =
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(512, Box::new(Clock::new(512))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let table = RowTable::create(&TableFiles::new(pool, file, Arc::new(MemoryLog::new())), &TYPES, options).unwrap();
    table.zones.begin_warm();
    let ids = (1..=3).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    (table, ids)
}

fn shown(row: &Row) -> Vec<String> {
    row.iter().map(|v| format!("{v:?}")).collect()
}

/// What `Twins::set` gives of a write refused with `code`.
fn refused(code: Code) -> String {
    format!("{code:?}")
}

/// Two tables alike, the first written by patches and the second whole.
struct Twins {
    patched: RowTable,
    whole: RowTable,
    ids: Vec<RowId>,
}

impl Twins {
    fn new(options: TableOptions) -> Twins {
        let ((patched, ids), (whole, _)) = (table(options), table(options));
        Twins { patched, whole, ids }
    }

    fn begin(&self, tx: u64, snapshot: u64) {
        self.patched.begin(tx, snapshot).unwrap();
        self.whole.begin(tx, snapshot).unwrap();
    }

    /// Row `id` given `values` in their columns by `tx`: by a row that
    /// holds those alone in the first table, told the columns, and by
    /// the row whole in the second. What each gave, checked alike: the
    /// row replaced, read by the columns set, as text, or the code of
    /// its refusal alone. The image the first gave of the row after is
    /// checked the image of the row the second wrote.
    fn set(&self, tx: u64, id: RowId, values: &[(usize, Value)]) -> Vec<String> {
        let set: Vec<usize> = values.iter().map(|(c, _)| *c).collect();
        let mut sparse = vec![Value::Null; TYPES.len()];
        let mut full = self.whole.get(tx, id).unwrap().unwrap_or_else(|| sparse.clone());
        for (c, v) in values {
            sparse[*c] = v.clone();
            full[*c] = v.clone();
        }
        let gave = |written: crate::error::Result<Row>| written.map_or_else(|e| vec![format!("{:?}", e.code)], |old| shown(&old));
        let (mut image, mut of_row) = (AfterImage::new(), AfterImage::new());
        let patch = Patch { set: &set, image: &mut image };
        let patched = self.patched.replace_set(tx, id, Some(&sparse), (Some(patch), &set));
        // The row after as its log record holds it is the whole row's.
        of_row.put_row(&full, &set);
        assert!(patched.is_err() || image.parts() == of_row.parts(), "row {id}, columns {set:?}: the image");
        let (patched, whole) = (gave(patched), gave(self.whole.replace_set(tx, id, Some(&full), (None, &set))));
        assert_eq!(patched, whole, "row {id}, columns {set:?}");
        patched
    }

    /// The newest version of row `id` in each table, checked the same
    /// bytes, its stamps and the place of the version before it too.
    fn same_version(&self, id: RowId) -> Vec<u8> {
        let (patched, whole) = (self.patched.tree.get(&key(id)).unwrap().unwrap(), self.whole.tree.get(&key(id)).unwrap().unwrap());
        assert_eq!(patched, whole, "row {id}");
        patched
    }

    /// Row `id` as `tx` reads it in each table, checked alike.
    fn same_row(&self, tx: u64, id: RowId) -> Vec<String> {
        let (patched, whole) = (self.patched.get(tx, id).unwrap(), self.whole.get(tx, id).unwrap());
        assert_eq!(patched.as_ref().map(shown), whole.as_ref().map(shown), "row {id}");
        patched.map(|row| shown(&row)).unwrap_or_default()
    }
}

/// Verifies: [DESIGN] RULES.md D916, D29: a row patched leaves the
/// version a row written whole leaves, byte for byte, its maker, the
/// place of the version before it in the undo store and its record: of a
/// committed version, of the transaction's own version patched again, of
/// a varying column grown, a CHARACTER one made null and numbers set
/// together. The row replaced is given by the columns read. Committed,
/// both read alike; a patch rolled back puts back the version before,
/// the same bytes in both.
#[test]
fn a_patch_leaves_the_version_a_whole_write_leaves() {
    let twins = Twins::new(TableOptions::default());
    let id = twins.ids[1];
    twins.begin(10, 9);
    let longer = Value::Char("a text grown past what it was".into());
    let old = twins.set(10, id, &[(1, longer)]);
    assert_eq!(old[1], format!("{:?}", row(2)[1]), "the column read");
    assert_eq!(old[0], format!("{:?}", Value::Null), "a column not read");
    let first = twins.same_version(id);
    assert!(Version::parse(&first).unwrap().older.is_some(), "the committed version went to the undo store");
    twins.set(10, id, &[(0, int(7)), (3, int(70))]);
    twins.set(10, id, &[(2, Value::Null), (1, Value::Char(String::new()))]);
    assert_ne!(twins.same_version(id), first);
    assert_eq!(twins.same_row(10, id)[0], format!("{:?}", int(7)));
    twins.patched.commit(10, 11);
    twins.whole.commit(10, 11);
    assert_eq!(twins.same_row(NO_TRANSACTION, id), shown(&vec![int(7), Value::Char(String::new()), Value::Null, int(70)]));
    let committed = twins.same_row(NO_TRANSACTION, id);
    twins.begin(20, 12);
    twins.set(20, id, &[(3, int(1))]);
    twins.patched.rollback(20).unwrap();
    twins.whole.rollback(20).unwrap();
    twins.same_version(id);
    assert_eq!(twins.same_row(NO_TRANSACTION, id), committed, "rolled back");
}

/// Verifies: [DESIGN] RULES.md D916, D29: a patch is refused as a write
/// whole is. Of a row another open transaction changed, and of one a
/// transaction committed a change to after the writer's snapshot, with
/// SQLCODE -911; of a row deleted and of no row, with SQLCODE -910.
/// Nothing is written then: the version is the one a whole write left.
#[test]
fn a_patch_is_refused_as_a_whole_write_is() {
    let twins = Twins::new(TableOptions::default());
    let id = twins.ids[0];
    twins.begin(10, 9);
    twins.begin(11, 9);
    twins.set(10, id, &[(3, int(5))]);
    assert_eq!(twins.set(11, id, &[(3, int(6))]), [refused(Code::Conflict)], "another's open change");
    twins.patched.commit(10, 12);
    twins.whole.commit(10, 12);
    assert_eq!(twins.set(11, id, &[(3, int(6))]), [refused(Code::Conflict)], "committed after its snapshot");
    twins.same_version(id);
    twins.begin(20, 13);
    twins.patched.delete(20, twins.ids[2]).unwrap();
    twins.whole.delete(20, twins.ids[2]).unwrap();
    assert_eq!(twins.set(20, twins.ids[2], &[(3, int(6))]), [refused(Code::Api)], "deleted");
    assert_eq!(twins.set(20, 999, &[(3, int(6))]), [refused(Code::Api)], "no such row");
    twins.same_version(twins.ids[2]);
}

/// The values a record of `table` keeps out of line in row `id`'s newest
/// version.
fn outside(table: &RowTable, id: RowId) -> usize {
    let newest = table.tree.get(&key(id)).unwrap().unwrap();
    table.format.outside(Version::parse(&newest).unwrap().record).unwrap().len()
}

/// Verifies: [DESIGN] RULES.md D916, D31: a record that keeps a value out
/// of line, or would once changed, is not patched. The row is written
/// whole, as it was before patches, and reads as the same row written
/// whole reads. Of another column set beside a value out of line, which
/// stays out; of a short value made long, which goes out; and of a long
/// one made short, which comes back in line. A write outside any
/// transaction, told the columns set, writes the row whole too.
#[test]
fn a_record_with_a_value_out_of_line_is_written_whole() {
    let twins = Twins::new(TableOptions { out_of_line: Some(10), ..TableOptions::default() });
    let (id, long) = (twins.ids[0], Value::Char("x".repeat(60)));
    twins.begin(10, 9);
    twins.set(10, id, &[(1, long.clone())]);
    assert_eq!((outside(&twins.patched, id), outside(&twins.whole, id)), (1, 1), "a short value made long goes out");
    twins.set(10, id, &[(3, int(8))]);
    assert_eq!((outside(&twins.patched, id), outside(&twins.whole, id)), (1, 1), "it stays out");
    assert_eq!(twins.same_row(10, id), shown(&vec![int(1), long, row(1)[2].clone(), int(8)]));
    twins.set(10, id, &[(1, Value::Char("short".into()))]);
    assert_eq!((outside(&twins.patched, id), outside(&twins.whole, id)), (0, 0), "a long value made short comes back");
    twins.same_version(id);
    twins.patched.commit(10, 11);
    twins.whole.commit(10, 11);
    twins.set(NO_TRANSACTION, twins.ids[1], &[(0, int(4)), (1, Value::Null)]);
    assert_eq!(twins.same_row(NO_TRANSACTION, twins.ids[1]), shown(&vec![int(4), Value::Null, row(2)[2].clone(), int(20)]));
}

/// Verifies: [DESIGN] RULES.md D916, D835: a patch widens its row's zone
/// to the value it set in a column the zones keep, decoded of the record
/// it wrote: a scan told a condition on that value reads the zone, as
/// after the same row written whole, and not before.
#[test]
fn a_patch_widens_its_rows_zone() {
    let twins = Twins::new(TableOptions::default());
    let id = twins.ids[1];
    let reads = |table: &RowTable, n: i128| {
        let equal = Conjunction::of(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Eq, value: int(n) } });
        table.zones.ranges(Some(&equal)).iter().any(|&(first, end)| first <= id && end.is_none_or(|e| id < e))
    };
    assert!(!reads(&twins.patched, 9) && !reads(&twins.whole, 9));
    twins.begin(10, 9);
    twins.set(10, id, &[(1, Value::Char("only the text".into()))]);
    assert!(!reads(&twins.patched, 9), "a column the zones do not keep");
    twins.set(10, id, &[(0, int(9))]);
    assert!(reads(&twins.patched, 9) && reads(&twins.whole, 9));
}
