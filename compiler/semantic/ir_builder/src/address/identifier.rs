use rayc_ir::address::Address;
use rayc_typed_ast::{
    name_binding::Source, typed_expr::identifier::Identifier, typed_function::TypedFunction,
};

use crate::{address::LowerAddress, builder::Builder};

impl LowerAddress<Identifier> for Builder {
    fn lower_address(
        &mut self,
        typed_function: &TypedFunction,
        identifier: &Identifier,
    ) -> Address {
        todo!("Huge refactor")
    }
}
