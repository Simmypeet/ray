use rayc_ir::address::Address;
use rayc_typed_ast::{
    function::Function as TypedFunction, name_binding::Source, typed_expr::identifier::Identifier,
};

use crate::{address::LowerAddress, builder::Builder};

impl LowerAddress<Identifier> for Builder {
    fn lower_address(
        &mut self,
        typed_function: &TypedFunction,
        identifier: &Identifier,
    ) -> Address {
        match typed_function.get_name_binding(identifier.name_binding()).source() {
            Source::Variable(variable_id) => self
                .source_variable(*variable_id)
                .map_or_else(|| self.error_address(), |ir_id| self.variable_address(ir_id)),
            Source::Parameter(parameter_id) => self.parameter_address(*parameter_id),
        }
    }
}
