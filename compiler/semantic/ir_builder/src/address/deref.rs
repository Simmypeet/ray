use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::deref::Deref;

use crate::{address::LowerAddress, builder::Builder, context::LoweringContext};

impl LowerAddress<Deref> for Builder {
    fn lower_address(&mut self, context: &LoweringContext<'_>, deref: &Deref) -> Address {
        let pointer = self.lower_expression_by_id(context, deref.pointee());
        self.dereference_address(pointer)
    }
}
