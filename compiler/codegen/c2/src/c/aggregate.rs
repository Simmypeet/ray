//! Rendering of aggregate `struct` definitions.

use std::fmt::{self, Display};

use rayc_mono_ir::ty::{AggregateType, HandlerLayout, MonoType};

use crate::c::{
    name::{AggregateName, FieldName},
    ty::{Declaration, FunctionDeclaration},
};

/// The member every empty aggregate carries, since C forbids empty structs.
const UNIT_MEMBER: &str = "    uint8_t _unit;\n";

/// The `struct` definition of one aggregate layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AggregateDefinition<'a> {
    aggregate: &'a AggregateType,
    /// The operation slots, present exactly for effect-handler aggregates.
    handler_layout: Option<&'a HandlerLayout>,
}

impl<'a> AggregateDefinition<'a> {
    pub(crate) const fn new(
        aggregate: &'a AggregateType,
        handler_layout: Option<&'a HandlerLayout>,
    ) -> Self {
        Self { aggregate, handler_layout }
    }

    fn write_handler_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let layout = self
            .handler_layout
            .expect("an effect-handler aggregate should have a resolved handler layout");
        if layout.operations().is_empty() {
            formatter.write_str(UNIT_MEMBER)?;
        }

        // Each operation is a closure: an environment and a callback.
        for operation in layout.operations() {
            let signature = operation.signature();
            let environment = FieldName::OperationEnvironment(operation.operation_id());
            let environment_type = signature
                .parameter_types()
                .first()
                .expect("an operation signature should have an environment parameter");
            writeln!(formatter, "    {};", Declaration::new(environment_type, &environment))?;

            let function = FieldName::OperationFunction(operation.operation_id());
            writeln!(formatter, "    {};", FunctionDeclaration::pointer(signature, &function))?;
        }
        Ok(())
    }
}

impl Display for AggregateDefinition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "struct {} {{", AggregateName::of(self.aggregate))?;
        match self.aggregate {
            AggregateType::EffectHandler(_) => self.write_handler_members(formatter)?,
            AggregateType::Tuple(tuple) => {
                write_positional_members(formatter, tuple.fields(), FieldName::Tuple)?;
            }
            AggregateType::Environment(environment) => {
                write_positional_members(
                    formatter,
                    environment.captures(),
                    FieldName::Environment,
                )?;
            }
            AggregateType::Struct(st) => {
                if st.fields().is_empty() {
                    formatter.write_str(UNIT_MEMBER)?;
                }
                for (field, ty) in st.fields() {
                    writeln!(
                        formatter,
                        "    {};",
                        Declaration::new(ty, &FieldName::Struct(*field))
                    )?;
                }
            }
        }
        formatter.write_str("};")
    }
}

/// Writes one member per element of a tuple or environment layout.
fn write_positional_members<T: AsRef<MonoType>>(
    formatter: &mut fmt::Formatter<'_>,
    types: &[T],
    field: fn(u32) -> FieldName,
) -> fmt::Result {
    if types.is_empty() {
        formatter.write_str(UNIT_MEMBER)?;
    }
    for (index, ty) in types.iter().enumerate() {
        let name = FieldName::positional(index, field);
        writeln!(formatter, "    {};", Declaration::new(ty.as_ref(), &name))?;
    }
    Ok(())
}
