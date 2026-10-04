//! Printing of operands and rvalues.

use std::fmt::{self, Display, Write as _};

use rayc_mono_ir::{
    instance::FunctionReference,
    operand::{FunctionOperand, Operand},
    rvalue::{AggregateEffectHandler, AggregateValue, Rvalue},
    ty::{AggregateType, EffectHandler, MonoType},
};

use crate::{
    c::{
        expr::{ConstantExpr, PlaceExpr, binary_operator_token, unary_operator_token},
        name::{
            AggregateName, ClosureName, DefinitionName, FieldName, FunctionName, NominalDropName,
        },
        ty::TypeName,
    },
    functions::Linkage,
    print::FunctionPrinter,
};

/// The C spelling of a referenced function.
#[derive(Debug, Clone, Copy)]
enum CalleeName<'a> {
    Local(FunctionName),
    Closure(ClosureName),
    NominalDrop(NominalDropName),
    Definition(DefinitionName),
    Extern(&'a str),
}

impl Display for CalleeName<'_> {
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

impl<'a> FunctionPrinter<'a> {
    pub(super) fn write_rvalue(
        &self,
        out: &mut String,
        value: &Rvalue,
        expected: Option<&MonoType>,
    ) -> fmt::Result {
        match value {
            Rvalue::Use(operand) => self.write_operand(out, operand, expected),
            Rvalue::AddressOf(address) => write!(out, "&({})", PlaceExpr(address.place())),
            Rvalue::Unary(unary) => {
                write!(out, "({}", unary_operator_token(unary.operator()))?;
                self.write_operand(out, unary.operand(), None)?;
                out.push(')');
                Ok(())
            }
            Rvalue::Binary(binary) => {
                out.push('(');
                self.write_operand(out, binary.left(), None)?;
                write!(out, " {} ", binary_operator_token(binary.operator()))?;
                self.write_operand(out, binary.right(), None)?;
                out.push(')');
                Ok(())
            }
            Rvalue::Cast(cast) => {
                write!(out, "(({})(", TypeName(cast.target()))?;
                self.write_operand(out, cast.operand(), None)?;
                out.push_str("))");
                Ok(())
            }
            Rvalue::Aggregate(aggregate) => self.write_aggregate_value(out, aggregate),
        }
    }

    /// Writes a compound literal that initializes every member.
    fn write_aggregate_value(&self, out: &mut String, value: &AggregateValue) -> fmt::Result {
        write!(out, "(({}){{ ", AggregateName::of(&aggregate_type_of(value)).typedef())?;
        let member_count = match value {
            AggregateValue::Tuple(tuple) => {
                let members = tuple.fields().iter().zip(tuple.ty().fields()).enumerate().map(
                    |(index, (operand, ty))| {
                        (FieldName::positional(index, FieldName::Tuple), operand, Some(&**ty))
                    },
                );
                self.write_members(out, members)?
            }
            AggregateValue::Environment(environment) => {
                let types = environment.ty().captures();
                let members = environment.fields().iter().zip(types).enumerate().map(
                    |(index, (operand, ty))| {
                        (FieldName::positional(index, FieldName::Environment), operand, Some(&**ty))
                    },
                );
                self.write_members(out, members)?
            }
            AggregateValue::Struct(st) => {
                let members = st.ty().fields().iter().map(|(field, ty)| {
                    let operand = st
                        .fields()
                        .get(field)
                        .expect("struct aggregate should initialize every field");
                    (FieldName::Struct(*field), operand, Some(&**ty))
                });
                self.write_members(out, members)?
            }
            AggregateValue::EffectHandler(handler) => {
                self.write_members(out, self.handler_members(handler))?
            }
        };
        if member_count == 0 {
            out.push_str("._unit = 0");
        }
        out.push_str(" })");
        Ok(())
    }

    /// The members of an effect-handler record in layout order: an
    /// environment and a callback per operation.
    fn handler_members<'v>(
        &self,
        handler: &'v AggregateEffectHandler,
    ) -> impl Iterator<Item = (FieldName, &'v Operand, Option<&'v MonoType>)> + 'v
    where
        'a: 'v,
    {
        let layout = self.aggregates.handler_layout(handler.effect());
        layout.operations().iter().flat_map(|operation| {
            let id = operation.operation_id();
            let slot = handler
                .slots()
                .get(&id)
                .expect("effect-handler aggregate should initialize every operation slot");
            let environment_type = operation.signature().parameter_types().first();
            [
                (
                    FieldName::OperationEnvironment(id),
                    slot.environment(),
                    environment_type.map(AsRef::as_ref),
                ),
                (FieldName::OperationFunction(id), slot.function(), None),
            ]
        })
    }

    /// Writes `.member = value` initializers, returning how many there were.
    fn write_members<'v>(
        &self,
        out: &mut String,
        members: impl Iterator<Item = (FieldName, &'v Operand, Option<&'v MonoType>)>,
    ) -> Result<usize, fmt::Error> {
        let mut count = 0;
        for (field, operand, expected) in members {
            if count != 0 {
                out.push_str(", ");
            }
            write!(out, ".{field} = ")?;
            self.write_operand(out, operand, expected)?;
            count += 1;
        }
        Ok(count)
    }

    pub(super) fn write_operand(
        &self,
        out: &mut String,
        operand: &Operand,
        expected: Option<&MonoType>,
    ) -> fmt::Result {
        match operand {
            Operand::Copy(place) => write!(out, "{}", PlaceExpr(place)),
            Operand::Constant(constant) => write!(out, "{}", ConstantExpr::new(constant, expected)),
            Operand::Function(function) => write!(out, "{}", self.callee_name(function)),
        }
    }

    fn callee_name(&self, operand: &FunctionOperand) -> CalleeName<'a> {
        match operand.function() {
            FunctionReference::Local(function_id) => {
                CalleeName::Local(self.fragment.name(*function_id))
            }
            FunctionReference::Closure(closure) => CalleeName::Closure(ClosureName::of(closure)),
            FunctionReference::NominalDrop(instance) => {
                CalleeName::NominalDrop(NominalDropName::of(instance))
            }
            FunctionReference::Global(instance) => {
                match self.functions.resolved_linkage(instance) {
                    Linkage::Internal => CalleeName::Definition(DefinitionName::of(instance)),
                    Linkage::Extern(name) => CalleeName::Extern(name),
                }
            }
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
