//! \[DESIGN\] The payload of a system table's one call, ROWS (RULES.md
//! D222): the rows a query of it reads, asked as it is read, a page at a
//! time.
//!
//! ```text
//! ROWS  user str | owner u8 | from u64 | max u32   -> rows | more u8
//! ```
//!
//! `from` counts the rows given before, so a program holds nothing
//! between pages; `more` is 1 while rows follow.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::plugin::wire::{put_rows, rows};
use crate::value::Value;

/// What ROWS asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The statement's user.
    pub user: String,
    /// Whether that user is the database's owner.
    pub owner: bool,
    /// The rows given before this page.
    pub from: u64,
    /// The most rows this page may hold.
    pub max: u32,
}

/// `p`, written.
pub fn put_page(out: &mut Vec<u8>, p: &Page) {
    out.put_str(&p.user);
    out.put_u8(u8::from(p.owner));
    out.put_u64(p.from);
    out.put_u32(p.max);
}

/// What `put_page` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn page(r: &mut Reader<'_>) -> Result<Page> {
    Ok(Page { user: r.str()?, owner: r.u8()? != 0, from: r.u64()?, max: r.u32()? })
}

/// ROWS's answer: the page's rows, and whether more follow.
///
/// # Errors
///
/// As `put_rows`.
pub fn put_paged(out: &mut Vec<u8>, given: &[Vec<Value>], more: bool) -> Result<()> {
    put_rows(out, given)?;
    out.put_u8(u8::from(more));
    Ok(())
}

/// What `put_paged` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn paged(r: &mut Reader<'_>) -> Result<(Vec<Vec<Value>>, bool)> {
    Ok((rows(r)?, r.u8()? != 0))
}
