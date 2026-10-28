//! \[DESIGN\] The storage foundation (RULES.md D30): local components that
//! keep bytes in files, each knowing nothing of the components built on it.
//! Built bottom up:
//!
//! * `io`: positional file I/O on every supported platform, behind a
//!   pluggable file system;
//! * `memory`: a file system in memory, for databases kept nowhere else;
//! * `measured`: statistics of that I/O, for the heuristics above;
//! * `encoding`: the pipeline of optional encodings (compression,
//!   encryption) that the block manager and the pager pass data through;
//! * `block`: the block manager, an append-only file of checksummed blocks;
//! * `pager`: a file of fixed-size checksummed pages, updated in place;
//! * `buffer`: the buffer pool, caching the pages of pager files and the
//!   blocks of block files in memory, with a pluggable eviction policy.
//!
//! Every component keeps statistics of its own work, which the components
//! above read to tune themselves.

pub mod block;
pub mod buffer;
pub mod encoding;
pub mod io;
pub mod measured;
pub mod memory;
pub mod pager;
pub mod shadow;
