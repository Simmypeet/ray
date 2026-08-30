use std::io::Write;

use rayc_ir::address::{Address, AddressRoot, Projection};

use crate::{
    expression::function_instance::FunctionInstance, identifier::Identifier, writer::Writer,
};

impl Writer<'_> {
    pub(crate) fn write_address(
        &mut self,
        address: &Address,
        function: FunctionInstance<'_>,
    ) -> std::io::Result<()> {
        match address.root() {
            AddressRoot::Error => {
                panic!("error address reached codegen, this should have been caught earlier")
            }
            AddressRoot::Variable(variable_id) => {
                write!(self, "{}", Identifier::var(variable_id))?;
            }
            AddressRoot::Parameter(parameter_id) => {
                write!(self, "{}", Identifier::param(parameter_id))?;
            }
            AddressRoot::LambdaParameter(parameter_id) => {
                assert!(
                    function.is_lambda(),
                    "compiler-internal invariant violation: lambda parameter used in a def"
                );
                write!(self, "{}", Identifier::lambda_param(parameter_id))?;
            }
            AddressRoot::OperationHandlerParameter(_) => {
                panic!("operation handler parameter reached C codegen before effect lowering")
            }
            AddressRoot::Capture(capture_id) => {
                let _ = function.lambda_context().get_capture(capture_id);
                write!(
                    self,
                    "{}->{}",
                    Identifier::lambda_typed_environment(),
                    Identifier::capture_field(capture_id)
                )?;
            }
            AddressRoot::Deref(expression_id) => {
                write!(self, "(*{})", Identifier::expr(expression_id))?;
            }
        }

        for projection in address.projections() {
            match projection {
                Projection::Tuple(index) => write!(self, ".{}", Identifier::tuple_elem(*index))?,
            }
        }

        Ok(())
    }
}
