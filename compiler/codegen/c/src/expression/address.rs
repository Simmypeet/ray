use std::io::Write;

use rayc_ir::address::{Address, AddressRoot, Projection};

use crate::{identifier::Identifier, writer::Writer};

impl Writer<'_> {
    pub(crate) fn write_address(&mut self, address: &Address) -> std::io::Result<()> {
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
            AddressRoot::LambdaParameter(_) => {
                panic!(
                    "lambda parameter address reached C codegen before closure codegen was added"
                )
            }
            AddressRoot::Capture(_) => {
                panic!("capture address reached C codegen before closure codegen was added")
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
