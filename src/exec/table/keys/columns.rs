//! \[DESIGN\] (RULES.md D1123) The keys of a table's groups given as the
//! columns of a batch, a group a row: each lane's vector as it is, of its
//! type, with no row of values made of a group, so what reads the groups
//! next reads them as it reads a scan's batches. Each column's value of a
//! row is the value the lane gives of that group (`Lane::value`). A lane
//! of texts gives its chunks as the one buffer a column holds
//! (`Chunks::joined`, RULES.md D1255).

use super::{Data, Keys, Lane};
use crate::storage::mask::mask_of;
use crate::storage::{Column, Floats, Ints, Nulls, Texts};

impl Lane {
    /// Its groups' values as a column, a group a row: its vector of
    /// integers or approximate numbers as it is, its texts in one buffer,
    /// its nulls a mask; of any other type the values.
    fn into_column(self) -> Column {
        let len = self.nulls.len();
        if matches!(self.data, Data::Datetimes(_) | Data::Bools(_)) {
            return Column::Values((0..len).map(|g| self.value(g)).collect());
        }
        let nulls = mask_of(&self.nulls);
        match self.data {
            Data::Ints { units, scale } => Column::Ints(Ints::new(units, scale, nulls)),
            Data::Floats(numbers) => Column::Floats(Floats::new(numbers, nulls)),
            Data::Texts(texts) => {
                let (text, ends) = texts.joined();
                Column::Texts(Texts::new(text, ends, nulls))
            }
            Data::Values { values, .. } => Column::Values(values),
            Data::Unset | Data::Datetimes(_) | Data::Bools(_) => Column::Nulls(Nulls::new(len)),
        }
    }
}

impl Keys {
    /// Its groups' keys as columns, a lane each, in the lanes' order.
    pub(in crate::exec) fn into_columns(self) -> Vec<Column> {
        self.lanes.into_iter().map(Lane::into_column).collect()
    }
}
