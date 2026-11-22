//! \[DESIGN\] The portable body of the CRC-32C kernel (RULES.md D354,
//! D572): CRC-32/ISCSI's register carried over bytes by its tables, sixteen
//! bytes a step (`hash::crc::Crc::update`). The register is reflected, as
//! the instructions of other sets keep it, with neither the starting value
//! nor the final exclusive or applied.

use std::sync::OnceLock;

use crate::hash::crc::{CRC32_ISCSI, Crc};

/// `Kernels::crc32c`: the register after `data`, from `register`.
pub fn crc32c(register: u32, data: &[u8]) -> u32 {
    static ISCSI: OnceLock<Option<Crc>> = OnceLock::new();
    let crc = ISCSI.get_or_init(|| Crc::new(CRC32_ISCSI).ok());
    // A reflected register of 32 bits stays in the low 32 bits.
    crc.as_ref().map_or(register, |c| u32::try_from(c.update(u64::from(register), data)).unwrap_or_default())
}
