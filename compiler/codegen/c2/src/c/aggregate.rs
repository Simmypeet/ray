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

use crate::{
    aggregates::AggregateRegistry,
    c::{
        name::{AggregateName, FieldName},
        ty::{ConstPointerDeclaration, Declaration, FunctionDeclaration},
    },
};

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

    /// Where the frame places each value it holds.
    pub(crate) fn layout(&self) -> &FrameLayout { &self.layout }

    /// The type of the member that holds `savable`.
    pub(crate) fn type_of(&self, savable: Savable) -> &Interned<MonoType> {
        self.types.get(savable)
    }

    /// Writes `{ state; prefix; union { start; suspension variants } }`. Every
    /// struct and union is anonymous; see [`FieldName::CoroutineFrameSavable`].
    ///
    /// A value of a zero-sized type has no member.
    fn write(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        layouts: &AggregateRegistry,
    ) -> fmt::Result {
        let state = FieldName::CoroutineFrameState;
        let state_type = FrameState::tag_type(&self.layout).mono_type();
        writeln!(formatter, "    {};", Declaration::new(&state_type, &state, layouts))?;

        // Placements order the prefix first, then the start variant, then the
        // suspension variants by block. Field order does not depend on the hash
        // iteration order of the layout, so neither does the generated struct.
        let mut members = self
            .layout
            .placements()
            .filter(|(savable, _)| !layouts.is_zero_sized(self.types.get(*savable)))
            .map(|(savable, placement)| (placement, savable))
            .collect::<Vec<_>>();
        members.sort_unstable();
        let prefix_len = members.partition_point(|(placement, _)| *placement == Placement::Prefix);
        let (prefix, variants) = members.split_at(prefix_len);

        for (_, savable) in prefix {
            self.write_member(formatter, "    ", *savable, layouts)?;
        }
        if variants.is_empty() {
            return Ok(());
        }

        // A variant without fields has no members to name, so it is left out.
        formatter.write_str("    union {\n")?;
        for variant in variants.chunk_by(|left, right| left.0 == right.0) {
            formatter.write_str("        struct {\n")?;
            for (_, savable) in variant {
                self.write_member(formatter, "            ", *savable, layouts)?;
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
        layouts: &AggregateRegistry,
    ) -> fmt::Result {
        let name = FieldName::CoroutineFrameSavable(savable);
        let declaration = Declaration::new(self.types.get(savable), &name, layouts);
        writeln!(formatter, "{indent}{declaration};")
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
    RunWithFrames(&'a RunWithLayout),
}

/// The `struct` definition of one aggregate layout, which is not zero-sized.
///
/// A member of a zero-sized type is left out. Every aggregate that is not
/// zero-sized has another member, so the struct is never empty.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AggregateDefinition<'a> {
    aggregate: &'a AggregateType,
    layout: NominalLayout<'a>,
    layouts: &'a AggregateRegistry,
}

impl<'a> AggregateDefinition<'a> {
    pub(crate) fn new(
        aggregate: &'a AggregateType,
        layout: NominalLayout<'a>,
        layouts: &'a AggregateRegistry,
    ) -> Self {
        assert!(
            !layouts.is_aggregate_zero_sized(aggregate),
            "a zero-sized aggregate is only declared"
        );
        Self { aggregate, layout, layouts }
    }

    /// Writes `{indent}type name;` unless `ty` is zero-sized.
    fn write_member(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        indent: &str,
        ty: &MonoType,
        name: &dyn Display,
    ) -> fmt::Result {
        if self.layouts.is_zero_sized(ty) {
            return Ok(());
        }
        writeln!(formatter, "{indent}{};", Declaration::new(ty, name, self.layouts))
    }

    fn write_handler_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectHandler(layout) = self.layout else {
            panic!("an effect-handler aggregate should have a resolved handler layout")
        };

        // Each operation is a closure: an environment and a callback.
        for operation in layout.operations() {
            let signature = operation.signature();
            let environment = FieldName::OperationEnvironment(operation.operation_id());
            let environment_type = signature
                .environment_slot()
                .expect("an operation signature should have an environment slot")
                .ty();
            self.write_member(formatter, "    ", &environment_type, &environment)?;

            let function = FieldName::OperationFunction(operation.operation_id());
            let function = FunctionDeclaration::pointer(signature, &function, self.layouts);
            writeln!(formatter, "    {function};")?;
        }
        Ok(())
    }

    /// Writes a member per capture, then a `const EffectHandler_E *` per
    /// handler record.
    fn write_environment_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::Environment(layout) = self.layout else {
            panic!("an environment should have a resolved environment layout")
        };

        for (capture, ty) in layout.captures() {
            let name = FieldName::EnvironmentCapture(capture);
            self.write_member(formatter, "    ", ty, &name)?;
        }
        for (record_id, record) in layout.handler_records() {
            let name = FieldName::EnvironmentHandlerRecord(record_id);
            let record = MonoType::Aggregate(AggregateType::EffectHandlerCallback(
                EffectHandlerCallback::new(record.effect().clone()),
            ));
            let record = ConstPointerDeclaration::new(&record, &name, self.layouts);
            writeln!(formatter, "    {record};")?;
        }
        Ok(())
    }

    fn write_struct_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::Struct(layout) = self.layout else {
            panic!("a struct should have a resolved struct layout")
        };
        for (field, ty) in layout.fields() {
            self.write_member(formatter, "    ", ty, &FieldName::Struct(*field))?;
        }
        Ok(())
    }

    fn write_frame_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::CoroutineFrame(members) = self.layout else {
            panic!("a coroutine frame should have a resolved frame layout")
        };

        members.write(formatter, self.layouts)
    }

    /// The members of a `coroutine_resume_t`, which overlap. The union is
    /// anonymous: every aggregate is spelled as a `struct`. Since the
    /// `coroutine_resume_t` is not zero-sized, one of them is not either.
    fn write_resume_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectRow(layout) = self.layout else {
            panic!("a coroutine resume type should have a resolved effect row layout")
        };

        formatter.write_str("    union {\n")?;
        for (id, variant) in layout.resume_variants() {
            let name = FieldName::CoroutineResumeValue(id);
            self.write_member(formatter, "        ", variant.ty(), &name)?;
        }
        formatter.write_str("    };\n")
    }

    /// `{ handler_environment; body; phase; union { clause frames } }`. The
    /// union is anonymous, so a clause frame is named by its operation alone.
    fn write_run_with_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::RunWithFrames(layout) = self.layout else {
            panic!("the frames of a `run ... with` should have a resolved layout")
        };

        if let Some(environment) = layout.handler_environment() {
            let environment = MonoType::Aggregate(AggregateType::Environment(environment.clone()));
            let name = FieldName::RunWithHandlerEnvironment;
            self.write_member(formatter, "    ", &environment, &name)?;
        }

        let body = MonoType::Aggregate(AggregateType::CoroutineFrame(layout.body().clone()));
        self.write_member(formatter, "    ", &body, &FieldName::RunWithBody)?;

        let phase_type = layout.phase_tag_type().mono_type();
        self.write_member(formatter, "    ", &phase_type, &FieldName::RunWithPhase)?;

        if layout.clauses().len() == 0 {
            return Ok(());
        }
        formatter.write_str("    union {\n")?;
        for clause in layout.clauses() {
            let frame =
                MonoType::Aggregate(AggregateType::CoroutineFrame(clause.coroutine().clone()));
            let name = FieldName::RunWithClause(clause.operation_id());
            self.write_member(formatter, "        ", &frame, &name)?;
        }
        formatter.write_str("    };\n")
    }

    /// `{ tag; union { arguments of each operation } }`. The union is left out
    /// when every operation's arguments are zero-sized.
    fn write_yield_members(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NominalLayout::EffectRow(layout) = self.layout else {
            panic!("a coroutine yield type should have a resolved effect row layout")
        };

        let tag = FieldName::CoroutineTag;
        let tag_type = layout.yield_tag_type().mono_type();
        self.write_member(formatter, "    ", &tag_type, &tag)?;

        let mut operations = layout
            .row_operations()
            .filter(|(_, operation)| !self.layouts.is_zero_sized(operation.arguments()))
            .peekable();
        if operations.peek().is_none() {
            return Ok(());
        }
        formatter.write_str("    union {\n")?;
        for (id, operation) in operations {
            let name = FieldName::CoroutineYieldOperation(id);
            self.write_member(formatter, "        ", operation.arguments(), &name)?;
        }
        formatter.write_str("    };\n")
    }

    /// `{ tag; union { yield_value; return_value } }`. The `coroutine_yield_t`
    /// has a tag, so the union is never empty.
    fn write_progress_members(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        progress: &CoroutineProgress,
    ) -> fmt::Result {
        let tag = FieldName::CoroutineTag;
        let tag_type = CoroutineProgress::TAG_TYPE.mono_type();
        self.write_member(formatter, "    ", &tag_type, &tag)?;

        let yielded = MonoType::Aggregate(AggregateType::CoroutineYield(CoroutineYield::new(
            progress.row().clone(),
        )));
        formatter.write_str("    union {\n")?;
        self.write_member(formatter, "        ", &yielded, &FieldName::CoroutineProgressYield)?;
        let return_name = FieldName::CoroutineProgressReturn;
        self.write_member(formatter, "        ", progress.return_type(), &return_name)?;
        formatter.write_str("    };\n")
    }

    /// Writes one member per element of a tuple layout.
    fn write_tuple_members(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        types: &[Interned<MonoType>],
    ) -> fmt::Result {
        for (index, ty) in types.iter().enumerate() {
            let name = FieldName::positional(index, FieldName::Tuple);
            self.write_member(formatter, "    ", ty, &name)?;
        }
        Ok(())
    }
}

impl Display for AggregateDefinition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "struct {} {{", AggregateName::of(self.aggregate))?;
        match self.aggregate {
            AggregateType::EffectHandlerCallback(_) => self.write_handler_members(formatter)?,
            AggregateType::Tuple(tuple) => self.write_tuple_members(formatter, tuple.fields())?,
            AggregateType::Environment(_) => self.write_environment_members(formatter)?,
            AggregateType::Struct(_) => self.write_struct_members(formatter)?,
            AggregateType::CoroutineFrame(_) => self.write_frame_members(formatter)?,
            AggregateType::CoroutineResume(_) => self.write_resume_members(formatter)?,
            AggregateType::CoroutineYield(_) => self.write_yield_members(formatter)?,
            AggregateType::CoroutineProgress(progress) => {
                self.write_progress_members(formatter, progress)?;
            }
            AggregateType::RunWithFrames(_) => self.write_run_with_members(formatter)?,
        }
        formatter.write_str("};")
    }
}
