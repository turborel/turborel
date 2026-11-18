//! \[EXTENSION\] Property graphs: path search over a graph's elements,
//! and the library of graph algorithms (`algo.rs`).
//!
//! Citations: `[GS §n]` is the project's property graph implementation
//! standard, section n, and its rules `Rn.m`; `[P1 §n]` is Deutsch et
//! al., "Graph Pattern Matching in GQL and SQL/PGQ" (SIGMOD 2022, arXiv
//! 2112.06217); `[P2 §n]` is Gheerbrant, Libkin, Peterfreund and Rogova,
//! "GQL and SQL/PGQ: Theoretical Models and Expressive Power" (PVLDB
//! 18(6), 2025, arXiv 2409.01102).

pub mod adjacency;
pub mod algo;
pub mod binding;
pub mod in_memory;
pub mod indexes;
pub mod limits;
pub mod matching;
pub mod nfa;
pub mod program;
pub mod run;
pub mod search;
pub mod view;
pub mod wcoj;
