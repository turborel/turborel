//! Test encodings: a run-length compression and a scrambling that plays
//! an encryption, for the tests of encodings over files and tables. Split
//! from common.rs (RULES.md R-SIZE-2).

use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::Code;

/// Run-length compression: a flag byte, then either the input as it is (0),
/// or (count, byte) pairs (1), whichever is shorter.
pub struct RunLength;

const RAW: u8 = 0;
const RUNS: u8 = 1;

impl Encoding for RunLength {
    fn id(&self) -> EncodingId {
        EncodingId(1001)
    }
    fn name(&self) -> &'static str {
        "test run-length"
    }
    fn overhead(&self, _: &str) -> usize {
        1
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        let mut runs = Vec::new();
        for chunk in input.chunk_by(|a, b| a == b) {
            for part in chunk.chunks(255) {
                runs.push(u8::try_from(part.len()).unwrap());
                runs.push(part[0]);
            }
        }
        if runs.len() < input.len() {
            out.push(RUNS);
            out.extend_from_slice(&runs);
        } else {
            out.push(RAW);
            out.extend_from_slice(input);
        }
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        match input.split_first() {
            Some((&RAW, data)) => out.extend_from_slice(data),
            Some((&RUNS, runs)) => {
                for pair in runs.chunks(2) {
                    out.extend(std::iter::repeat_n(pair[1], usize::from(pair[0])));
                }
            }
            _ => return Err(turborel::error::SqlError::new(Code::Corrupt, "not run-length encoded")),
        }
        Ok(())
    }
}

/// XORs each byte with a key stream derived from the context, and appends
/// the context, refusing to decode under another: what an authenticated
/// encryption would do, without the cryptography.
pub struct Scramble;

impl Encoding for Scramble {
    fn id(&self) -> EncodingId {
        EncodingId(1002)
    }
    fn name(&self) -> &'static str {
        "test scramble"
    }
    fn overhead(&self, _: &str) -> usize {
        8
    }
    fn confidential(&self) -> bool {
        true
    }
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        out.extend(input.iter().enumerate().map(|(i, &b)| b ^ key(context, i)));
        out.extend_from_slice(&context.to_le_bytes());
        Ok(())
    }
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        let (data, bound) = input.split_at(input.len().saturating_sub(8));
        if bound != context.to_le_bytes() {
            return Err(turborel::error::SqlError::new(Code::Corrupt, "scrambled for another place"));
        }
        out.extend(data.iter().enumerate().map(|(i, &b)| b ^ key(context, i)));
        Ok(())
    }
}

fn key(context: u64, i: usize) -> u8 {
    u8::try_from((context.wrapping_mul(31).wrapping_add(i as u64 * 7) + 0x5A) % 256).unwrap()
}
