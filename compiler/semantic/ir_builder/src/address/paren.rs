use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::paren::Paren;

use crate::{address::LowerAddress, builder::Builder, context::LoweringContext};

impl LowerAddress<Paren> for Builder {
    fn lower_address(&mut self, context: &LoweringContext<'_>, paren: &Paren) -> Address {
        self.lower_address_by_id(context, paren.expression())
    }
}
