//! Rendering of aggregate `struct` definitions.

use std::fmt::{self, Display};

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::{
    frame_layout::{FrameLayout, Placement},
    savable::Savable,
};
use rayc_mono_ir::{
    layout::{
        EffectHandlerCallbackLayout, EnvironmentLayout, StructLayout,
        coroutine::{EffectRowLayout, FrameFieldTypes, FrameState},
        run_with::RunWithLayout,
    },
    ty::{AggregateType, CoroutineProgress, CoroutineYield, EffectHandlerCallback, MonoType},
};

use crate::c::{
    name::{AggregateName, FieldName},
    ty::{ConstPointerDeclaration, Declaration, FunctionDeclaration},
};

/// The member every empty aggregate carries, since C forbids empty structs.
const UNIT_MEMBER: &str = "    uint8_t _unit;\n";

/// What defining a `coroutine_frame_t` takes: where each value is placed, and
/// its type.
#[derive(Debug, Clone)]
pub(crate) struct FrameMembers {
    layout: Interned<FrameLayout>,
    types: Interned<FrameFieldTypes>,
}

impl FrameMembers {
    pub(crate) const fn new(
        layout: Interned<FrameLayout>,
        types: Interned<FrameFieldTypes>,
    ) -> Self {
        Self { layout, types }
    }

    /// The type of every value the frame holds.
    pub(crate) fn types(&self) -> impl Iterator<Item = &Interned<MonoType>> { self.types.iter() }

    /// Writes `{ state; prefix; union { start; suspension variants } }`. Every
    /// struct and union is anonymous; see [`FieldName::CoroutineFrameSavable`].
    fn write(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = FieldName::CoroutineFrameState;
        let state_type = FrameState::tag_type(&self.layout).mono_type();
        writeln!(formatter, "    {};", Declaration::new(&state_type, &state))?;

        // Placements order the prefix first, then the start variant, then the
        // suspension variants by block. Field order does not depend on the hash
        // iteration order of the layout, so neither does the generated struct.
        let mut members = self
            .layout
            .placements()
            .map(|(savable, placement)| (placement, savable))
            .collect::<Vec<_>>();
        members.sort_unstable();
        let prefix_len = members.partition_point(|(placement, _)| *placement == Placement::Prefix);
        let (prefix, variants) = members.split_at(prefix_len);

        for (_, savable) in prefix {
            self.write_member(formatter, "    ", *savable)?;
        }
        if variants.is_empty() {
            return Ok(());
        }

        // A variant without fields has no members to name, so it is left out.
        formatter.write_str("    union {\n")?;
        for variant in variants.chunk_by(|left, right| left.0 == right.0) {
            formatter.write_str("        struct {\n")?;
            for (_, savable) in variant {
                self.write_member(formatter, "            ", *savable)?;
            }
            formatter.write_str("        };\n")?;
        }
        formatter.write_str("    };\n")
    }

    /// Writes the declaration of the member that holds `savable`.
    fn write_member(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        indent: &str,
        savable: Savable,
    ) -> fmt::Result {
        let name = FieldName::CoroutineFrameSavable(savable);
        writeln!(formatter, "{indent}{};", Declaration::new(self.types.get(savable), &name))
    }
}

/// The layout a nominal aggregate takes from a query.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NominalLayout<'a> {
    /// The aggregate spells its own fields.
    None,
    EffectHandler(&'a EffectHandlerCallbackLayout),
    Environment(&'a EnvironmentLayout),
    Struct(&'a StructLayout),
    CoroutineFrame(&'a FrameMembers),
    EffectRow(&'a EffectRowLayout),
    RunWithAwaitee(&'a RunWithLayout),
}

/// The `struct` definition of one aggregate layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AggregateDefinition<'a> {
    aggregate: &'a AggregateType,
    layout: NominalLayout<'a>,
}

impl<'a> AggregateDefinition<'a> {
    pub(crate) const fn new(aggregate: &'a AggregateType, layout: NominalLayout<'a>) -> Self {
        Self { aggregate, layout }
    }

    fn write_handler_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectHandler(layout) = self.layout else {
            panic!("an effect-handler aggregate should have a resolved handler layout")
        };
        if layout.operations().is_empty() {
            formatter.write_str(UNIT_MEMBER)?;
        }

        // Each operation is a closure: an environment and a callback.
        for operation in layout.operations() {
            let signature = operation.signature();
            let environment = FieldName::OperationEnvironment(operation.operation_id());
            let environment_type = signature
                .environment_slot()
                .expect("an operation signature should have an environment slot")
                .ty();
            writeln!(formatter, "    {};", Declaration::new(&environment_type, &environment))?;

            let function = FieldName::OperationFunction(operation.operation_id());
            writeln!(formatter, "    {};", FunctionDeclaration::pointer(signature, &function))?;
        }
        Ok(())
    }

    /// Writes a member per capture, then a `const EffectHandler_E *` per
    /// handler record.
    fn write_environment_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::Environment(layout) = self.layout else {
            panic!("an environment should have a resolved environment layout")
        };
        if layout.is_empty() {
            formatter.write_str(UNIT_MEMBER)?;
        }

        for (capture, ty) in layout.captures() {
            let name = FieldName::EnvironmentCapture(capture);
            writeln!(formatter, "    {};", Declaration::new(ty, &name))?;
        }
        for (record_id, record) in layout.handler_records() {
            let name = FieldName::EnvironmentHandlerRecord(record_id);
            let record = MonoType::Aggregate(AggregateType::EffectHandlerCallback(
                EffectHandlerCallback::new(record.effect().clone()),
            ));
            writeln!(formatter, "    {};", ConstPointerDeclaration::new(&record, &name))?;
        }
        Ok(())
    }

    fn write_struct_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::Struct(layout) = self.layout else {
            panic!("a struct should have a resolved struct layout")
        };
        if layout.fields().is_empty() {
            formatter.write_str(UNIT_MEMBER)?;
        }
        for (field, ty) in layout.fields() {
            writeln!(formatter, "    {};", Declaration::new(ty, &FieldName::Struct(*field)))?;
        }
        Ok(())
    }

    fn write_frame_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::CoroutineFrame(members) = self.layout else {
            panic!("a coroutine frame should have a resolved frame layout")
        };

        members.write(formatter)
    }

    /// The members of a `coroutine_resume_t`, which overlap. The union is
    /// anonymous: every aggregate is spelled as a `struct`.
    fn write_resume_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectRow(layout) = self.layout else {
            panic!("a coroutine resume type should have a resolved effect row layout")
        };

        formatter.write_str("    union {\n")?;
        for (id, variant) in layout.resume_variants() {
            let name = FieldName::CoroutineResumeValue(id);
            writeln!(formatter, "        {};", Declaration::new(variant.ty(), &name))?;
        }
        formatter.write_str("    };\n")
    }

    /// `{ handler_environment; body; phase; union { clause frames } }`. The
    /// union is anonymous, so a clause frame is named by its operation alone.
    fn write_run_with_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::RunWithAwaitee(layout) = self.layout else {
            panic!("a `run ... with` awaitee should have a resolved layout")
        };

        if let Some(environment) = layout.handler_environment() {
            let environment = MonoType::Aggregate(AggregateType::Environment(environment.clone()));
            let name = FieldName::RunWithHandlerEnvironment;
            writeln!(formatter, "    {};", Declaration::new(&environment, &name))?;
        }

        let body = MonoType::Aggregate(AggregateType::CoroutineFrame(layout.body().clone()));
        writeln!(formatter, "    {};", Declaration::new(&body, &FieldName::RunWithBody))?;

        let phase_type = layout.phase_tag_type().mono_type();
        writeln!(formatter, "    {};", Declaration::new(&phase_type, &FieldName::RunWithPhase))?;

        if layout.clauses().len() == 0 {
            return Ok(());
        }
        formatter.write_str("    union {\n")?;
        for clause in layout.clauses() {
            let frame =
                MonoType::Aggregate(AggregateType::CoroutineFrame(clause.coroutine().clone()));
            let name = FieldName::RunWithClause(clause.operation_id());
            writeln!(formatter, "        {};", Declaration::new(&frame, &name))?;
        }
        formatter.write_str("    };\n")
    }

    /// `{ tag; union { arguments of each operation } }`.
    fn write_yield_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectRow(layout) = self.layout else {
            panic!("a coroutine yield type should have a resolved effect row layout")
        };

        let tag = FieldName::CoroutineTag;
        let tag_type = layout.yield_tag_type().mono_type();
        writeln!(formatter, "    {};", Declaration::new(&tag_type, &tag))?;

        if layout.row_operations().len() == 0 {
            return Ok(());
        }
        formatter.write_str("    union {\n")?;
        for (id, operation) in layout.row_operations() {
            let name = FieldName::CoroutineYieldOperation(id);
            writeln!(formatter, "        {};", Declaration::new(operation.arguments(), &name))?;
        }
        formatter.write_str("    };\n")
    }
}

/// `{ tag; union { yield_value; return_value } }`.
fn write_progress_members(
    formatter: &mut fmt::Formatter<'_>,
    progress: &CoroutineProgress,
) -> fmt::Result {
    let tag = FieldName::CoroutineTag;
    let tag_type = CoroutineProgress::TAG_TYPE.mono_type();
    writeln!(formatter, "    {};", Declaration::new(&tag_type, &tag))?;

    let yielded = MonoType::Aggregate(AggregateType::CoroutineYield(CoroutineYield::new(
        progress.row().clone(),
    )));
    formatter.write_str("    union {\n")?;
    writeln!(
        formatter,
        "        {};",
        Declaration::new(&yielded, &FieldName::CoroutineProgressYield)
    )?;
    writeln!(
        formatter,
        "        {};",
        Declaration::new(progress.return_type(), &FieldName::CoroutineProgressReturn)
    )?;
    formatter.write_str("    };\n")
}

impl Display for AggregateDefinition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "struct {} {{", AggregateName::of(self.aggregate))?;
        match self.aggregate {
            AggregateType::EffectHandlerCallback(_) => self.write_handler_members(formatter)?,
            AggregateType::Tuple(tuple) => {
                write_positional_members(formatter, tuple.fields(), FieldName::Tuple)?;
            }
            AggregateType::Environment(_) => self.write_environment_members(formatter)?,
            AggregateType::Struct(_) => self.write_struct_members(formatter)?,
            AggregateType::CoroutineFrame(_) => self.write_frame_members(formatter)?,
            AggregateType::CoroutineResume(_) => self.write_resume_members(formatter)?,
            AggregateType::CoroutineYield(_) => self.write_yield_members(formatter)?,
            AggregateType::CoroutineProgress(progress) => {
                write_progress_members(formatter, progress)?;
            }
            AggregateType::RunWithAwaitee(_) => self.write_run_with_members(formatter)?,
        }
        formatter.write_str("};")
    }
}

/// Writes one member per element of a tuple layout.
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
