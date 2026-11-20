//! COUNT [X3.135-1986 §5.8], its accumulator counting the values that are
//! not null, of any batch form.

use super::{Accumulator, AggregateFunction, floats, ints};
use crate::error::{Code, Result, fail};
use crate::lexer::kw;
use crate::storage::{Floats, Ints, Texts};
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// COUNT(*) and COUNT(DISTINCT c). [X3.135-1986 §5.8 GR4a, GR4b] the cardinality.
pub(super) struct Count;

impl AggregateFunction for Count {
    fn name(&self) -> &'static str {
        kw::COUNT
    }
    // [X3.135-1986 §5.8 SR7] exact numeric with implementor-defined precision and scale 0.
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(CountAcc(0))
    }
}

struct CountAcc(i128);

impl Accumulator for CountAcc {
    fn update(&mut self, _v: &Value) -> Result<()> {
        self.0 = self.0.saturating_add(1);
        Ok(())
    }
    fn update_many(&mut self, values: &[Value], rows: Option<&[u32]>) -> Result<()> {
        let counted = match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(crate::storage::mask::index(r))).filter(|v| !v.is_null()).count(),
            None => values.iter().filter(|v| !v.is_null()).count(),
        };
        self.update_rows(counted)
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[u32]>) -> Result<()> {
        self.update_rows(ints::count(ints, rows))
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[u32]>) -> Result<()> {
        self.update_rows(floats::count(floats, rows))
    }
    /// Its texts counted, none made a value.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[u32]>) -> Result<()> {
        let counted = match rows {
            Some(rows) => rows.iter().filter(|&&n| texts.get(crate::storage::mask::index(n)).is_some()).count(),
            None => (0..texts.len()).filter(|&n| texts.get(n).is_some()).count(),
        };
        self.update_rows(counted)
    }
    fn update_rows(&mut self, rows: usize) -> Result<()> {
        self.0 = self.0.saturating_add(i128::try_from(rows).unwrap_or(i128::MAX));
        Ok(())
    }
    /// The count, its 16 bytes.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(&self.0.to_le_bytes());
        true
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let n = <[u8; 16]>::try_from(state).map(i128::from_le_bytes);
        self.0 = self.0.saturating_add(n.or_else(|_| fail(Code::Corrupt, "[DESIGN] a state of COUNT is not 16 bytes"))?);
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}
