//! Printing of operands and rvalues.

use std::fmt::{self, Write as _};

use rayc_mono_ir::{
    MonoFunctionInstance,
    operand::{Constant, Operand},
    rvalue::{Reinterpret, Rvalue},
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
    print::FunctionPrinter,
};

impl FunctionPrinter<'_, '_> {
    pub(super) async fn write_rvalue(&mut self, out: &mut String, value: &Rvalue) -> fmt::Result {
        match value {
            Rvalue::Use(operand) => self.write_operand(out, operand).await,
            // a place that holds nothing has no storage of its own
            Rvalue::AddressOf(address) => {
                if self.is_place_zero_sized(address.place()).await {
                    out.push_str(ZERO_SIZED_ADDRESS);
                    return Ok(());
                }
                write!(out, "&{}", PlaceExpr(address.place()))
            }
            Rvalue::Unary(unary) => {
                write!(out, "({}", unary_operator_token(unary.operator()))?;
                self.write_operand(out, unary.operand()).await?;
                out.push(')');
                Ok(())
            }
            Rvalue::Binary(binary) => {
                out.push('(');
                self.write_operand(out, binary.left()).await?;
                write!(out, " {} ", binary_operator_token(binary.operator()))?;
                self.write_operand(out, binary.right()).await?;
                out.push(')');
                Ok(())
            }
            Rvalue::Cast(cast) => {
                self.context.use_type(cast.target()).await;
                let target = TypeName::new(cast.target(), self.context.aggregate_registry());
                write!(out, "(({target})(")?;
                self.write_operand(out, cast.operand()).await?;
                out.push_str("))");
                Ok(())
            }
            Rvalue::Reinterpret(reinterpret) => self.write_reinterpret(out, reinterpret).await,
            // the aggregates are defined before every function, so `sizeof`
            // sees a complete type; a value that holds nothing needs no memory
            Rvalue::HeapAllocate(ty) => {
                if self.context.is_zero_sized(ty).await {
                    out.push_str(ZERO_SIZED_ADDRESS);
                    return Ok(());
                }
                let ty = TypeName::new(ty, self.context.aggregate_registry());
                write!(out, "ray_alloc(sizeof({ty}))")
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
    async fn write_reinterpret(
        &mut self,
        out: &mut String,
        reinterpret: &Reinterpret,
    ) -> fmt::Result {
        self.context.use_type(reinterpret.target()).await;
        let is_source_zero_sized = self.context.is_zero_sized(reinterpret.source()).await;
        let target = TypeName::new(reinterpret.target(), self.context.aggregate_registry());
        if is_source_zero_sized {
            return write!(out, "(({target}){{ 0 }})");
        }

        let source = TypeName::new(reinterpret.source(), self.context.aggregate_registry());
        write!(out, "((union {{ {source} source; {target} target; }}){{ .source = ")?;
        self.write_operand(out, reinterpret.operand()).await?;
        out.push_str(" }).target");
        Ok(())
    }

    pub(super) async fn write_operand(
        &mut self,
        out: &mut String,
        operand: &Operand,
    ) -> fmt::Result {
        match operand {
            Operand::Copy(place) => write!(out, "{}", PlaceExpr(place)),
            Operand::Constant(constant) => self.write_operand_constant(out, constant).await,
            // naming a function is the only way to refer to one, so this is
            // where the functions the body needs are found
            Operand::Function(instance) => {
                self.context.use_function(instance);
                write!(out, "{}", self.link_name(instance).await)
            }
        }
    }

    pub(super) async fn write_operand_constant(
        &mut self,
        out: &mut String,
        constant: &Constant,
    ) -> fmt::Result {
        self.use_constant_type(constant).await;
        write!(out, "{}", ConstantExpr::new(constant, self.context.aggregate_registry()))
    }

    /// Records the aggregate types a constant is spelled with.
    async fn use_constant_type(&mut self, constant: &Constant) {
        match constant {
            Constant::NullPointer(ty) => self.context.use_type(ty).await,
            Constant::Unit
            | Constant::EmptyAggregate(_)
            | Constant::Bool(_)
            | Constant::Int8(_)
            | Constant::Int16(_)
            | Constant::Int32(_)
            | Constant::Int64(_)
            | Constant::Isize(_)
            | Constant::Uint8(_)
            | Constant::Uint16(_)
            | Constant::Uint32(_)
            | Constant::Uint64(_)
            | Constant::Usize(_)
            | Constant::Float32(_)
            | Constant::CInt(_)
            | Constant::CStr(_) => {}
        }
    }

    /// The name `instance` is linked under, which an `extern def` takes from
    /// its source.
    async fn link_name(&self, instance: &MonoFunctionInstance) -> LinkName {
        match instance {
            MonoFunctionInstance::ExternDef(def_id) => {
                LinkName::Extern(self.context.engine().get_name(*def_id).await)
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
