use rayc_ir::address::Address;
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::deref::Deref};

use crate::{address::LowerAddress, builder::Builder};

impl LowerAddress<Deref> for Builder {
    fn lower_address(&mut self, typed_function: &TypedFunction, deref: &Deref) -> Address {
        let pointer = self.lower_expression_by_id(typed_function, deref.pointee());
        self.dereference_address(pointer)
    }
}
