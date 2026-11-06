//! Printing of operands and rvalues.

use std::fmt::{self, Display, Write as _};

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    instance::FunctionReference,
    operand::{Constant, Operand},
    rvalue::{Reinterpret, Rvalue},
};

use crate::{
    c::{
        expr::{ConstantExpr, PlaceExpr, binary_operator_token, unary_operator_token},
        name::{ClosureName, CoroutineResumeName, DefinitionName, FunctionName, NominalDropName},
        ty::TypeName,
    },
    print::FunctionPrinter,
    program::Linkage,
};

/// The C spelling of a referenced function.
#[derive(Debug, Clone)]
enum CalleeName {
    Local(FunctionName),
    Closure(ClosureName),
    NominalDrop(NominalDropName),
    CoroutineResume(CoroutineResumeName),
    Definition(DefinitionName),
    Extern(Interned<str>),
}

impl Display for CalleeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(name) => name.fmt(formatter),
            Self::Closure(name) => name.fmt(formatter),
            Self::NominalDrop(name) => name.fmt(formatter),
            Self::CoroutineResume(name) => name.fmt(formatter),
            Self::Definition(name) => name.fmt(formatter),
            Self::Extern(name) => formatter.write_str(name),
        }
    }
}

impl FunctionPrinter<'_, '_> {
    pub(super) async fn write_rvalue(&mut self, out: &mut String, value: &Rvalue) -> fmt::Result {
        match value {
            Rvalue::Use(operand) => self.write_operand(out, operand).await,
            Rvalue::AddressOf(address) => write!(out, "&{}", PlaceExpr(address.place())),
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
                self.program.use_type(cast.target()).await;
                write!(out, "(({})(", TypeName(cast.target()))?;
                self.write_operand(out, cast.operand()).await?;
                out.push_str("))");
                Ok(())
            }
            Rvalue::Reinterpret(reinterpret) => self.write_reinterpret(out, reinterpret).await,
        }
    }

    /// A compound literal of a union of both types, whose bytes are written as
    /// one and read as the other, which C defines even for types that differ
    /// in size.
    async fn write_reinterpret(
        &mut self,
        out: &mut String,
        reinterpret: &Reinterpret,
    ) -> fmt::Result {
        self.program.use_type(reinterpret.source()).await;
        self.program.use_type(reinterpret.target()).await;
        write!(
            out,
            "((union {{ {} source; {} target; }}){{ .source = ",
            TypeName(reinterpret.source()),
            TypeName(reinterpret.target()),
        )?;
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
            Operand::Function(reference) => {
                self.program.use_function(reference);
                write!(out, "{}", self.callee_name(reference).await)
            }
        }
    }

    pub(super) async fn write_operand_constant(
        &mut self,
        out: &mut String,
        constant: &Constant,
    ) -> fmt::Result {
        self.use_constant_type(constant).await;
        write!(out, "{}", ConstantExpr::new(constant, self.program.unit_type()))
    }

    /// Records the aggregate types a constant is spelled with.
    async fn use_constant_type(&mut self, constant: &Constant) {
        match constant {
            Constant::Unit => self.program.use_aggregate(self.program.unit_type().clone()).await,
            Constant::NullPointer(ty) => self.program.use_type(ty).await,
            Constant::EmptyAggregate(ty) => self.program.use_aggregate(ty.clone()).await,
            Constant::Bool(_)
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

    async fn callee_name(&self, reference: &FunctionReference) -> CalleeName {
        match reference {
            FunctionReference::Local(function_id) => {
                CalleeName::Local(self.fragment.name(*function_id))
            }
            FunctionReference::Closure(closure) => CalleeName::Closure(ClosureName::of(closure)),
            FunctionReference::NominalDrop(instance) => {
                CalleeName::NominalDrop(NominalDropName::of(instance))
            }
            FunctionReference::CoroutineResume(instance) => {
                CalleeName::CoroutineResume(CoroutineResumeName::of(instance))
            }
            FunctionReference::Global(instance) => match self.program.linkage(instance).await {
                Linkage::Internal => CalleeName::Definition(DefinitionName::of(instance)),
                Linkage::Extern(name) => CalleeName::Extern(name),
            },
        }
    }
}
