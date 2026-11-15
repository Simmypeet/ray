//! Printing of operands and rvalues.

use rayc_mono_ir::{
    MonoFunctionInstance,
    layout::repr::Repr,
    operand::{Constant, Operand},
    rvalue::{AddressOf, Reinterpret, Rvalue},
    ty::MonoType,
};
use rayc_symbol::name::get_name;

use crate::{
    c::{
        expr::{
            ConstantExpr, PlaceExpr, ZERO_SIZED_ADDRESS, binary_operator_token,
            unary_operator_token,
        },
        name::{FunctionName, LinkName},
        ty::TypeName,
    },
    function_print::FunctionPrinter,
};

impl FunctionPrinter<'_, '_> {
    pub(super) async fn write_rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Use(operand) => self.write_operand(operand).await,
            Rvalue::AddressOf(address) => self.write_address_of(address).await,
            Rvalue::Unary(unary) => {
                self.writer.push(format_args!("({}", unary_operator_token(unary.operator())));
                self.write_operand(unary.operand()).await;
                self.writer.push(")");
            }
            Rvalue::Binary(binary) => {
                self.writer.push("(");
                self.write_operand(binary.left()).await;
                self.writer.push(format_args!(" {} ", binary_operator_token(binary.operator())));
                self.write_operand(binary.right()).await;
                self.writer.push(")");
            }
            Rvalue::Cast(cast) => {
                self.writer.push("((");
                self.writer.write(&TypeName::new(cast.target())).await;
                self.writer.push(")(");
                self.write_operand(cast.operand()).await;
                self.writer.push("))");
            }
            Rvalue::Reinterpret(reinterpret) => self.write_reinterpret(reinterpret).await,
            // the aggregates are defined before every function, so `sizeof`
            // sees a complete type; a value that holds nothing needs no memory
            Rvalue::HeapAllocate(ty) => {
                if self.writer.is_zero_sized(ty).await {
                    self.writer.push(ZERO_SIZED_ADDRESS);
                    return;
                }
                self.writer.push("ray_alloc(sizeof(");
                self.writer.write(&TypeName::new(ty)).await;
                self.writer.push("))");
            }
        }
    }

    /// The address of a place, as a pointer to the place's type.
    ///
    /// A place that holds nothing has no storage of its own. A place of a
    /// transparent aggregate is spelled as the member it stores, so its
    /// address is cast to a pointer to the aggregate's struct, which is how a
    /// pointer to the aggregate is spelled.
    async fn write_address_of(&mut self, address: &AddressOf) {
        let (ty, projections) = self.spell_place(address.place()).await;
        let place = PlaceExpr::new(address.place().local(), &projections);
        match self.writer.repr(&ty).await {
            Repr::ZeroSized => self.writer.push(ZERO_SIZED_ADDRESS),
            Repr::Transparent(_) => {
                let engine = self.writer.engine();
                let pointer = MonoType::new_pointer(ty, address.mutability(), engine);
                self.writer.push("((");
                self.writer.write(&TypeName::new(&pointer)).await;
                self.writer.push(")&");
                self.writer.write(&place).await;
                self.writer.push(")");
            }
            Repr::Direct => {
                self.writer.push("&");
                self.writer.write(&place).await;
            }
        }
    }

    /// A compound literal of a union of both types, whose bytes are written as
    /// one and read as the other, which C defines even for types that differ
    /// in size.
    ///
    /// The target is not zero-sized, since its assignment would be left out.
    /// A zero-sized source holds nothing, so what is read from the target
    /// cannot depend on it: the target is zero instead.
    async fn write_reinterpret(&mut self, reinterpret: &Reinterpret) {
        let target = TypeName::new(reinterpret.target());
        if self.writer.is_zero_sized(reinterpret.source()).await {
            self.writer.push("((");
            self.writer.write(&target).await;
            self.writer.push("){ 0 })");
            return;
        }

        self.writer.push("((union { ");
        self.writer.write(&TypeName::new(reinterpret.source())).await;
        self.writer.push(" source; ");
        self.writer.write(&target).await;
        self.writer.push(" target; }){ .source = ");
        self.write_operand(reinterpret.operand()).await;
        self.writer.push(" }).target");
    }

    pub(super) async fn write_operand(&mut self, operand: &Operand) {
        match operand {
            Operand::Copy(place) => self.write_place(place).await,
            Operand::Constant(constant) => self.write_operand_constant(constant).await,
            // naming a function is the only way to refer to one, so this is
            // where the functions the body needs are found
            Operand::Function(instance) => {
                self.callees.push(instance.clone());
                let name = self.link_name(instance).await;
                self.writer.push(name);
            }
        }
    }

    pub(super) async fn write_operand_constant(&mut self, constant: &Constant) {
        self.writer.write(&ConstantExpr::new(constant)).await;
    }

    /// The name `instance` is linked under, which an `extern def` takes from
    /// its source.
    async fn link_name(&self, instance: &MonoFunctionInstance) -> LinkName {
        match instance {
            MonoFunctionInstance::ExternDef(def_id) => {
                LinkName::Extern(self.writer.engine().get_name(*def_id).await)
            }
            MonoFunctionInstance::Body(_)
            | MonoFunctionInstance::NominalDrop(_)
            | MonoFunctionInstance::CoroutineResume(_)
            | MonoFunctionInstance::CoroutineDrop(_) => {
                LinkName::Generated(FunctionName::of(instance))
            }
        }
    }
}
