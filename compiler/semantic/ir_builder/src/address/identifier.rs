use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::identifier::Identifier;

use crate::{address::LowerAddress, builder::Builder, context::LoweringContext};

impl LowerAddress<Identifier> for Builder {
    fn lower_address(&mut self, context: &LoweringContext<'_>, identifier: &Identifier) -> Address {
        let source = context.name_binding_source(identifier.name_binding());
        self.source_address(source)
    }
}
