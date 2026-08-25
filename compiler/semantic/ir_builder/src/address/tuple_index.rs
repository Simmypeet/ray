use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::tuple_index::TupleIndex;

use crate::{address::LowerAddress, builder::Builder, context::LoweringContext};

impl LowerAddress<TupleIndex> for Builder {
    fn lower_address(
        &mut self,
        context: &LoweringContext<'_>,
        tuple_index: &TupleIndex,
    ) -> Address {
        let mut address = self.lower_address_by_id(context, tuple_index.operand());
        self.project_tuple(&mut address, tuple_index.index());
        address
    }
}
