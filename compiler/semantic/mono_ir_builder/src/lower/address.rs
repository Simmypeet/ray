use rayc_ir::address::{Address, AddressRoot, Projection as IRProjection};
use rayc_mono_ir::place::{FieldIndex, Place};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    pub(super) fn lower_address(&self, address: &Address) -> Place {
        let mut place = match address.root() {
            AddressRoot::Error => {
                panic!("compiler-internal invariant violation: error address reached MonoIR")
            }
            AddressRoot::Variable(variable) => self.variable_place(variable),
            AddressRoot::Parameter(parameter) => self.parameter_place(parameter),
            AddressRoot::ClosureParameter(parameter) => self.closure_parameter_place(parameter),
            AddressRoot::OperationHandlerParameter(parameter) => {
                self.operation_parameter_place(parameter)
            }
            AddressRoot::Capture(capture) => self.capture_place(capture),
        };
        for projection in address.projections() {
            let projector = self.project(&place);
            place = match projection {
                IRProjection::Tuple(index) => projector.tuple_field(FieldIndex::from_usize(*index)),
                IRProjection::Field(id) => projector.struct_field(*id),
                // References and raw pointers have the same representation.
                IRProjection::Deref | IRProjection::RawDeref => projector.dereference(),
            };
        }
        place
    }
}
