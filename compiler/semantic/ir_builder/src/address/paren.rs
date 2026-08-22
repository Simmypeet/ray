use rayc_ir::address::Address;
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::paren::Paren};

use crate::{address::LowerAddress, builder::Builder};

impl LowerAddress<Paren> for Builder {
    fn lower_address(&mut self, typed_function: &TypedFunction, paren: &Paren) -> Address {
        self.lower_address_by_id(typed_function, paren.expression())
    }
}
