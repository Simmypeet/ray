//! Printing of operands and rvalues.

use std::fmt::{self, Display, Write as _};

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    instance::FunctionReference,
    operand::{Constant, Operand},
    rvalue::{AggregateValue, Rvalue},
    ty::{AggregateType, EffectHandler},
};

use crate::{
    c::{
        expr::{ConstantExpr, PlaceExpr, binary_operator_token, unary_operator_token},
        name::{
            AggregateName, ClosureName, DefinitionName, FieldName, FunctionName, NominalDropName,
        },
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
    Definition(DefinitionName),
    Extern(Interned<str>),
}

impl Display for CalleeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(name) => name.fmt(formatter),
            Self::Closure(name) => name.fmt(formatter),
            Self::NominalDrop(name) => name.fmt(formatter),
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
            Rvalue::Aggregate(aggregate) => self.write_aggregate_value(out, aggregate).await,
        }
    }

    /// Writes a compound literal that initializes every member.
    async fn write_aggregate_value(
        &mut self,
        out: &mut String,
        value: &AggregateValue,
    ) -> fmt::Result {
        let ty = aggregate_type_of(value);
        write!(out, "(({}){{ ", AggregateName::of(&ty).typedef())?;
        self.program.use_aggregate(ty).await;

        let mut members = 0;
        match value {
            AggregateValue::Tuple(tuple) => {
                for (index, operand) in tuple.fields().iter().enumerate() {
                    self.write_member(
                        out,
                        &mut members,
                        FieldName::positional(index, FieldName::Tuple),
                        operand,
                    )
                    .await?;
                }
            }
            AggregateValue::Environment(environment) => {
                for (index, operand) in environment.fields().iter().enumerate() {
                    let field = FieldName::positional(index, FieldName::Environment);
                    self.write_member(out, &mut members, field, operand).await?;
                }
            }
            AggregateValue::Struct(st) => {
                for (field, operand) in st.fields() {
                    self.write_member(out, &mut members, FieldName::Struct(*field), operand)
                        .await?;
                }
            }
            // Each operation slot is a closure: an environment and a callback.
            AggregateValue::EffectHandler(handler) => {
                for (operation, slot) in handler.slots() {
                    let environment = FieldName::OperationEnvironment(*operation);
                    self.write_member(out, &mut members, environment, slot.environment()).await?;
                    let function = FieldName::OperationFunction(*operation);
                    self.write_member(out, &mut members, function, slot.function()).await?;
                }
            }
        }

        if members == 0 {
            out.push_str("._unit = 0");
        }
        out.push_str(" })");
        Ok(())
    }

    /// Writes the `.field = value` initializer after the `members` before it.
    async fn write_member(
        &mut self,
        out: &mut String,
        members: &mut usize,
        field: FieldName,
        operand: &Operand,
    ) -> fmt::Result {
        if *members != 0 {
            out.push_str(", ");
        }
        *members += 1;
        write!(out, ".{field} = ")?;
        self.write_operand(out, operand).await
    }

    pub(super) async fn write_operand(
        &mut self,
        out: &mut String,
        operand: &Operand,
    ) -> fmt::Result {
        match operand {
            Operand::Copy(place) => write!(out, "{}", PlaceExpr(place)),
            Operand::Constant(constant) => {
                self.use_constant_type(constant).await;
                write!(out, "{}", ConstantExpr::new(constant, self.program.unit_type()))
            }
            Operand::Function(reference) => {
                self.program.use_function(reference);
                write!(out, "{}", self.callee_name(reference).await)
            }
        }
    }

    /// Records the aggregate types a constant is spelled with.
    async fn use_constant_type(&mut self, constant: &Constant) {
        match constant {
            Constant::Unit => self.program.use_aggregate(self.program.unit_type().clone()).await,
            Constant::NullPointer(ty) => self.program.use_type(ty).await,
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
            FunctionReference::Global(instance) => match self.program.linkage(instance).await {
                Linkage::Internal => CalleeName::Definition(DefinitionName::of(instance)),
                Linkage::Extern(name) => CalleeName::Extern(name),
            },
        }
    }
}

/// The aggregate type an aggregate value constructs.
fn aggregate_type_of(value: &AggregateValue) -> AggregateType {
    match value {
        AggregateValue::Tuple(tuple) => AggregateType::Tuple(tuple.ty().clone()),
        AggregateValue::Environment(environment) => {
            AggregateType::Environment(environment.ty().clone())
        }
        AggregateValue::EffectHandler(handler) => {
            AggregateType::EffectHandler(EffectHandler::new(handler.effect().clone()))
        }
        AggregateValue::Struct(st) => AggregateType::Struct(st.ty().clone()),
    }
}
