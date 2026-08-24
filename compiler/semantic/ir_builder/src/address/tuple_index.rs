use rayc_ir::address::Address;
use rayc_typed_ast::{typed_function::TypedFunction as TypedFunction, typed_expr::tuple_index::TupleIndex};

use crate::{address::LowerAddress, builder::Builder};

impl LowerAddress<TupleIndex> for Builder {
    fn lower_address(
        &mut self,
        typed_function: &TypedFunction,
        tuple_index: &TupleIndex,
    ) -> Address {
        let mut address = self.lower_address_by_id(typed_function, tuple_index.operand());
        self.project_tuple(&mut address, tuple_index.index());
        address
    }
}
