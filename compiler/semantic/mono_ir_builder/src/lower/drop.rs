//! Lowers drops. A generated nominal `Drop.drop` has no source function, so
//! what does not read the source function is available on the [`Emitter`].

use qbice::storage::intern::Interned;
use rayc_ir::cfg::{AddressDrop, ExprDiscard};
use rayc_mono_ir::{
    operand::Operand,
    place::{FieldIndex, Place},
};
use rayc_type::ty::Ty;

use crate::{
    function_lowering::{FunctionLowering, emitter::Emitter},
    resolver::InstanceCallable,
};

impl FunctionLowering<'_> {
    /// Drops the unused value of an expression statement with the dictionary
    /// selected during type checking.
    pub(super) async fn lower_expr_discard(&mut self, discard: &ExprDiscard) {
        // `Drop.drop` returns unit, which is itself unused, so every call
        // writes to one scratch temporary.
        let destination = self.scratch_unit();

        let value = self.expression_place(discard.expression());
        self.lower_drop(value, discard.drop_instance(), destination).await;
    }

    /// Drops the value held in a place with the dictionary selected by drop
    /// elaboration. The place is passed to the drop directly, so no
    /// temporary holds the moved value.
    pub(super) async fn lower_address_drop(&mut self, drop: &AddressDrop) {
        // `Drop.drop` returns unit, which is unused, so every call writes to
        // one scratch temporary.
        let destination = self.scratch_unit();

        let value = self.lower_address(drop.address());
        self.lower_drop(value, drop.drop_instance(), destination).await;
    }
}

impl Emitter<'_> {
    /// Drops `value` with the implementation selected by `dictionary`. Every
    /// emitted call writes its unit result to `destination`, and passes on
    /// `handlers` to what it calls.
    pub(crate) async fn lower_drop(
        &mut self,
        value: Place,
        dictionary: &Interned<Ty>,
        destination: Place,
    ) {
        match self.resolver().resolve_drop_call(dictionary).await {
            InstanceCallable::Definition(callee) => {
                self.lower_call_with_args(callee, vec![Operand::Copy(value)], destination).await;
            }
            InstanceCallable::TupleDrop(element_instances) => {
                Box::pin(self.lower_tuple_drop(&value, &element_instances, destination)).await;
            }
            InstanceCallable::ClosureDrop(capture_instances) => {
                Box::pin(self.lower_closure_drop(&value, &capture_instances, destination)).await;
            }
            InstanceCallable::NominalDrop(instance) => {
                self.lower_nominal_drop_call(instance, vec![Operand::Copy(value)], destination);
            }
            InstanceCallable::NoOp => {}
            InstanceCallable::Closure(_, _, _) | InstanceCallable::Continuation(_) => {
                panic!("a Drop dictionary cannot call a closure or a continuation")
            }
        }
    }

    /// Expands a built-in tuple `Drop` call into the selected element calls.
    pub(crate) async fn lower_tuple_drop(
        &mut self,
        tuple: &Place,
        element_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        // Destroy fields in reverse declaration order, matching scope teardown.
        for (index, element_instance) in element_instances.iter().enumerate().rev() {
            let field = self.project(tuple).tuple_field(FieldIndex::from_usize(index));
            self.lower_drop(field, element_instance, destination.clone()).await;
        }
    }

    /// Expands a built-in closure `Drop` call into the selected capture calls.
    pub(crate) async fn lower_closure_drop(
        &mut self,
        closure: &Place,
        capture_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        // A closure value is its inline environment, whose fields are the
        // captures. Destroy them in reverse order, matching tuple Drop.
        for (index, capture_instance) in capture_instances.iter().enumerate().rev() {
            let capture = self.project(closure).environment_field(FieldIndex::from_usize(index));
            self.lower_drop(capture, capture_instance, destination.clone()).await;
        }
    }
}
