//! Unit tests: runs of blocks framed by their caller appended as they are,
//! a block of parts read back as one, and blocks near one another read
//! ahead together (RULES.md D1146, R-TEST-1).

use std::path::Path;
use std::sync::Arc;

use super::super::{BlockFile, BlockId, BlockOptions};
use super::frame_onto;
use crate::disk::encoding::Pipeline;
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};

fn hash() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

fn blocks() -> BlockFile {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    fs.create_dir_all(Path::new("/d")).unwrap();
    BlockFile::create(&*fs, Path::new("/d/b"), hash(), Pipeline::none(), BlockOptions::default()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D1146: a run framed by `frame_onto` and
/// appended whole keeps each block at its offset within the run past the
/// run's start; a block of parts reads back as the parts joined; blocks
/// read near one another are each one's payload, in order; a run that
/// does not end where a block does is refused, nothing appended.
#[test]
fn framed_runs_and_parts_read_back_as_written() {
    let file = blocks();
    let payloads: [&[u8]; 3] = [b"first", &[7; 5000], b"third"];
    let mut run = Vec::new();
    let mut offsets = Vec::new();
    for p in payloads {
        offsets.push(u64::try_from(run.len()).unwrap());
        frame_onto(&*hash(), p, &mut run).unwrap();
    }
    let at = file.append_frames(&run).unwrap();
    for (p, off) in payloads.iter().zip(&offsets) {
        assert_eq!(file.read(BlockId(at.0 + off)).unwrap(), p.to_vec());
    }
    let parts = file.append_parts(&[b"head|", &[9; 3000], b"|tail"]).unwrap();
    let joined = [b"head|".to_vec(), vec![9; 3000], b"|tail".to_vec()].concat();
    assert_eq!(file.read(parts).unwrap(), joined);
    let ids: Vec<u64> = offsets.iter().map(|o| at.0 + o).chain([parts.0]).collect();
    let wanted: Vec<Option<Vec<u8>>> = payloads.iter().map(|p| Some(p.to_vec())).chain([Some(joined)]).collect();
    assert_eq!(file.read_near(&ids), wanted);
    let end = file.end();
    assert!(file.append_frames(&run[..run.len() - 1]).is_err(), "a cut run");
    assert_eq!(file.end(), end, "nothing appended");
}
