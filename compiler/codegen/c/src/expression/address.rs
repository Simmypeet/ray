use std::io::Write;

use rayc_ir::address::{Address, AddressRoot, Projection};

use crate::writer::Writer;

impl Writer<'_> {
    pub(crate) fn write_address(&mut self, address: &Address) -> std::io::Result<()> {
        match address.root() {
            AddressRoot::Error => {
                panic!("error address reached codegen, this should have been caught earlier")
            }
            AddressRoot::Variable(variable_id) => {
                write!(self, "ray_var_{:X}", variable_id.index())?;
            }
            AddressRoot::Parameter(parameter_id) => {
                write!(self, "ray_param_{:X}", parameter_id.index())?;
            }
            AddressRoot::Deref(expression_id) => {
                write!(self, "(*ray_expr_{:X})", expression_id.index())?;
            }
        }

        for projection in address.projections() {
            match projection {
                Projection::Tuple(index) => write!(self, ".elem{index:X}")?,
            }
        }

        Ok(())
    }
}
