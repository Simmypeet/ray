//! The types of the members that place projections select.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoRunWithInstance,
    layout::{
        build_enum_layout, build_environment_layout, build_handler_layout, build_struct_layout,
        coroutine::{
            FrameState, build_effect_row_layout, build_frame_field_types, build_frame_layout,
        },
        run_with::build_run_with_layout,
    },
    place::{
        CoroutineFrame, CoroutineProgress as ProgressMember, CoroutineYield as YieldMember,
        Projection, RunWithFrames,
    },
    ty::{AggregateType, CoroutineProgress, CoroutineYield, MonoType},
};
use rayc_qbice::TrackedEngine;

/// The type of the place that `projection` selects from a place of type `ty`.
///
/// Members are looked up through the layout queries of the aggregate's kind,
/// which the engine caches. Members that no layout holds, such as tags, are
/// interned here.
pub(crate) async fn projected_type(
    engine: &TrackedEngine,
    ty: &MonoType,
    projection: Projection,
) -> Interned<MonoType> {
    const NOT_AN_AGGREGATE: &str = "a member is projected from an aggregate";

    // dereferencing is the only projection of a non-aggregate
    let aggregate = match projection {
        Projection::Dereference => return ty.assert_as_pointer().pointee().clone(),
        Projection::EnvironmentCapture(_)
        | Projection::EnvironmentHandlerRecord(_)
        | Projection::TupleFieldIndex(_)
        | Projection::StructFieldIndex(_)
        | Projection::VariantPayload(_)
        | Projection::OperationRecordEnvironmentField(_)
        | Projection::OperationRecordFunctionPointerField(_)
        | Projection::CoroutineFrame(_)
        | Projection::CoroutineResumeValue(_)
        | Projection::CoroutineProgress(_)
        | Projection::CoroutineYield(_)
        | Projection::RunWithFrames(_) => ty.as_aggregate().expect(NOT_AN_AGGREGATE),
    };

    match (projection, aggregate) {
        (Projection::TupleFieldIndex(index), AggregateType::Tuple(tuple)) => {
            let index = usize::try_from(index.index()).expect("a field index fits in usize");
            tuple.fields()[index].clone()
        }
        (Projection::StructFieldIndex(field), AggregateType::Struct(instance)) => engine
            .build_struct_layout(instance.clone())
            .await
            .fields()
            .get(&field)
            .expect("a struct should have the field it is projected to")
            .clone(),

        (Projection::VariantPayload(variant), AggregateType::Enum(instance)) => engine
            .build_enum_layout(instance.clone())
            .await
            .variant(variant)
            .payload()
            .expect("only a variant with a payload has one to project")
            .clone(),

        (Projection::EnvironmentCapture(capture), AggregateType::Environment(instance)) => {
            engine.build_environment_layout(instance.clone()).await.capture_type(capture).clone()
        }
        (Projection::EnvironmentHandlerRecord(record), AggregateType::Environment(instance)) => {
            let layout = engine.build_environment_layout(instance.clone()).await;
            MonoType::new_handler_pointer(layout.handler_record(record).effect().clone(), engine)
        }

        // the callback of an operation is a pair: the environment it closes
        // over, and the function it calls with it
        (
            Projection::OperationRecordEnvironmentField(operation),
            AggregateType::EffectHandlerCallback(handler),
        ) => {
            let layout = engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
            let environment = layout
                .operation(operation)
                .signature()
                .environment_slot()
                .expect("an operation signature should have an environment slot");
            engine.intern(environment.ty())
        }
        (
            Projection::OperationRecordFunctionPointerField(operation),
            AggregateType::EffectHandlerCallback(handler),
        ) => {
            let layout = engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
            engine
                .intern(MonoType::FunctionPointer(layout.operation(operation).signature().clone()))
        }

        (Projection::CoroutineFrame(member), AggregateType::CoroutineFrame(instance)) => {
            frame_member_type(engine, instance, member).await
        }
        (Projection::CoroutineResumeValue(variant), AggregateType::CoroutineResume(resume)) => {
            engine.build_effect_row_layout(resume.row().clone()).await[variant].ty().clone()
        }
        (Projection::CoroutineProgress(member), AggregateType::CoroutineProgress(progress)) => {
            match member {
                ProgressMember::Tag => engine.intern(CoroutineProgress::TAG_TYPE.mono_type()),
                ProgressMember::ReturnPayload => progress.return_type().clone(),
                ProgressMember::YieldPayload => MonoType::new_aggregate(
                    AggregateType::CoroutineYield(CoroutineYield::new(progress.row().clone())),
                    engine,
                ),
            }
        }
        (Projection::CoroutineYield(member), AggregateType::CoroutineYield(yielded)) => {
            let layout = engine.build_effect_row_layout(yielded.row().clone()).await;
            match member {
                YieldMember::Tag => engine.intern(layout.yield_tag_type().mono_type()),
                YieldMember::OperationPayload(operation) => layout[operation].arguments().clone(),
            }
        }

        (Projection::RunWithFrames(member), AggregateType::RunWithFrames(instance)) => {
            run_with_member_type(engine, instance, member).await
        }

        (projection, aggregate) => {
            panic!("{projection:?} does not select a member of {aggregate:?}")
        }
    }
}

/// The type of `member` of the frame of the coroutine `instance`: its state
/// tag, or a value it saves across a suspension.
async fn frame_member_type(
    engine: &TrackedEngine,
    instance: &MonoCoroutineInstance,
    member: CoroutineFrame,
) -> Interned<MonoType> {
    match member {
        CoroutineFrame::Tag => {
            let layout = engine.build_frame_layout(instance.clone()).await;
            engine.intern(FrameState::tag_type(&layout).mono_type())
        }
        CoroutineFrame::Savable(savable) => {
            engine.build_frame_field_types(instance.clone()).await.get(savable).clone()
        }
    }
}

/// The type of `member` of the frames of the `run ... with` `instance`.
async fn run_with_member_type(
    engine: &TrackedEngine,
    instance: &MonoRunWithInstance,
    member: RunWithFrames,
) -> Interned<MonoType> {
    let layout = engine.build_run_with_layout(instance.clone()).await;
    let frame = |instance: &MonoCoroutineInstance| {
        MonoType::new_aggregate(AggregateType::CoroutineFrame(instance.clone()), engine)
    };

    match member {
        RunWithFrames::HandlerEnvironment => {
            let environment = layout
                .handler_environment()
                .expect("only a `run ... with` with clauses has a handler environment");
            MonoType::new_aggregate(AggregateType::Environment(environment.clone()), engine)
        }
        RunWithFrames::Body => frame(layout.body()),
        RunWithFrames::Phase => engine.intern(layout.phase_tag_type().mono_type()),
        RunWithFrames::Clause(operation) => frame(layout.clause(operation).coroutine()),
    }
}
