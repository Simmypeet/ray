use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::field_access::FieldAccess;

use crate::{address::LowerAddress, builder::Builder, context::LoweringContext};

impl LowerAddress<FieldAccess> for Builder {
    fn lower_address(
        &mut self,
        context: &LoweringContext<'_>,
        expression: &FieldAccess,
    ) -> Address {
        let mut address = self.lower_address_by_id(context, expression.operand());
        self.project_field(&mut address, expression.field());
        address
    }
}
