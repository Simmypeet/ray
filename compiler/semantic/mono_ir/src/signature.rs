//! Calling signatures of concrete functions.
//!
//! A caller only needs a callee's interface, so the signature is a query of
//! its own rather than something read off the callee's lowered body. Callers
//! therefore depend on the declaration, not the implementation, of what they
//! call.
//!
//! In callback style, a function receives its environment first and a handler
//! record per effect last; see [`FunctionSignature`]. A `coroutine_resume`
//! receives its frame and what it is resumed with instead.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_ir::{
    get_ir,
    ir_function::{IRContext, IRFunctionID},
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_solver::Solver;
use rayc_symbol::{
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};
use rayc_type::signature::{get_effect_row, get_return_type};

use crate::{
    MonoClosureInstance, MonoCoroutineInstance, MonoDefInstance, MonoEnvironmentInstance,
    ty::{
        AggregateType, CoroutineProgress, CoroutineResume, EffectRow, EnvironmentSlot,
        FunctionSignature, MonoType, PointerMutability, ReturnType, lower_type,
    },
};

/// Derives the callback-style signature of a concrete definition instance,
/// from its declaration alone, so that an `extern def` has one too.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FunctionSignature>)]
#[extend(name = get_callback_def_signature, by_val)]
pub struct CallbackDefSignatureKey {
    /// The definition instance whose interface is requested.
    pub instance: MonoDefInstance,
}

/// How a definition's kind shapes its calling convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefKind {
    /// A source definition outside an instance.
    Free,
    /// A source definition implementing a trait member in an instance.
    InstanceMember,
    /// A function provided by the C environment.
    Extern,
}

impl DefKind {
    fn of(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Def => Self::Free,
            SymbolKind::InstanceDef => Self::InstanceMember,
            SymbolKind::ExternDef => Self::Extern,

            SymbolKind::Effect
            | SymbolKind::EffectOperation
            | SymbolKind::Enum
            | SymbolKind::Instance
            | SymbolKind::Marker
            | SymbolKind::MarkerImplementation
            | SymbolKind::Module
            | SymbolKind::Struct
            | SymbolKind::Trait
            | SymbolKind::TraitType
            | SymbolKind::InstanceType
            | SymbolKind::TraitDef
            | SymbolKind::Variant => panic!("{kind:?} symbol has no calling signature"),
        }
    }

    /// Source definitions receive a handler for each effect they perform;
    /// C functions cannot perform Ray effects.
    const fn receives_handlers(self) -> bool {
        match self {
            Self::Free | Self::InstanceMember => true,
            Self::Extern => false,
        }
    }

    /// Only extern functions spell a unit result as C `void`.
    const fn returns_unit_as_void(self) -> bool {
        match self {
            Self::Extern => true,
            Self::Free | Self::InstanceMember => false,
        }
    }

    /// The symbol table records a variadic marker only for free and extern
    /// definitions.
    const fn may_be_variadic(self) -> bool {
        match self {
            Self::Free | Self::Extern => true,
            Self::InstanceMember => false,
        }
    }
}

#[executor(config = Config)]
async fn callback_def_signature_executor(
    key: &CallbackDefSignatureKey,
    engine: &TrackedEngine,
) -> Interned<FunctionSignature> {
    let def_id = key.instance.def_id();
    let substitution = key.instance.substitution();
    let kind = DefKind::of(engine.get_symbol_kind(def_id).await);
    let solver = Solver::without_givens(engine.clone()).await;

    // Source parameters, then a handler record per effect.
    let mut parameter_types = Vec::new();
    for (_, parameter) in engine.get_parameter_map(def_id).await.iter() {
        parameter_types.push(lower_type(&solver, parameter.ty(), substitution).await);
    }

    let effects = if kind.receives_handlers() {
        EffectRow::new(&solver, &engine.get_effect_row(def_id).await, substitution).await
    } else {
        EffectRow::new_empty(engine)
    };

    let return_type_raw =
        lower_type(&solver, &engine.get_return_type(def_id).await, substitution).await;

    let return_type = if kind.returns_unit_as_void() && return_type_raw.is_unit() {
        ReturnType::Void
    } else {
        ReturnType::Value(
            lower_type(&solver, &engine.get_return_type(def_id).await, substitution).await,
        )
    };

    let parameter_types = engine.intern_unsized(parameter_types);
    let variadic = kind.may_be_variadic() && engine.is_variadic_def(def_id).await;

    engine.intern(
        FunctionSignature::builder()
            .parameter_types(parameter_types)
            .return_type(return_type)
            .variadic(variadic)
            .handler_record_slots(effects)
            .engine(engine)
            .build(),
    )
}

/// Derives the callback-style signature of a function in the IR of a concrete
/// definition instance: the definition itself, the body of one of its
/// closures, the body of a `run ... with`, or the clause of one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FunctionSignature>)]
#[extend(name = get_callback_function_signature, by_val)]
pub struct CallbackFunctionSignatureKey {
    /// The definition instance whose IR holds the function.
    pub owner: MonoDefInstance,
    /// The function in the IR of `owner`.
    pub function_id: IRFunctionID,
}

#[executor(config = Config)]
async fn callback_function_signature_executor(
    key: &CallbackFunctionSignatureKey,
    engine: &TrackedEngine,
) -> Interned<FunctionSignature> {
    let owner = &key.owner;
    let substitution = owner.substitution();
    let source = engine.get_ir(owner.def_id()).await;
    let function = source.get_function(key.function_id);
    let solver = Solver::without_givens(engine.clone()).await;

    // What the kind of the function decides: its source parameters, how it
    // receives its environment, and whether it receives handler records.
    let (parameters, environment_slot, receives_handler_records) = match function.context() {
        // The definition implements exactly its declared signature, which is
        // what its callers spell.
        IRContext::Def => return engine.get_callback_def_signature(owner.clone()).await,

        // A closure body receives its environment by value, as callers of the
        // closure's `Def.call` pass it.
        IRContext::Closure(closure) => {
            let closure_id = source
                .closures()
                .find_map(|(closure, function)| (function == key.function_id).then_some(closure))
                .expect("a closure body should be registered as a closure");
            let environment = MonoEnvironmentInstance::Closure(MonoClosureInstance::new(
                owner.clone(),
                closure_id,
            ));
            let parameters = closure.parameters().map(|(_, p)| p.ty().clone()).collect();
            (parameters, EnvironmentSlot::ByValue(environment), true)
        }

        IRContext::Thunk(_) => (Vec::new(), EnvironmentSlot::Opaque, true),

        // An operation handler is called through the slot of a handler record,
        // which has no handler-record slots, so it finds its handler records in
        // its environment instead.
        IRContext::OperationHandler(handler) => {
            let parameters = handler.parameters().map(|(_, p)| p.ty().clone()).collect::<Vec<_>>();
            (parameters, EnvironmentSlot::Opaque, false)
        }
    };

    // Lower the source parameters, the result and the effects in the owner's
    // substitution.
    let mut parameter_types = Vec::with_capacity(parameters.len());
    for parameter in &parameters {
        parameter_types.push(lower_type(&solver, parameter, substitution).await);
    }
    let return_type = function.context().return_ty(owner.def_id(), engine).await;
    let return_type = lower_type(&solver, &return_type, substitution).await;
    let handler_record_slots = if receives_handler_records {
        let effect = source.effect_of(key.function_id, engine).await;
        Some(EffectRow::new(&solver, &effect, substitution).await)
    } else {
        None
    };

    engine.intern(
        FunctionSignature::builder()
            .parameter_types(engine.intern_unsized(parameter_types))
            .return_type(ReturnType::Value(return_type))
            .environment_slot(environment_slot)
            .maybe_handler_record_slots(handler_record_slots)
            .engine(engine)
            .build(),
    )
}

/// The interface of a concrete coroutine, derived from the signature of the
/// function it suspends.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct CoroutineSignature {
    row: EffectRow,
    progress: CoroutineProgress,
    resume_signature: FunctionSignature,
}

impl CoroutineSignature {
    /// The effects the coroutine may perform.
    #[must_use]
    pub const fn row(&self) -> &EffectRow { &self.row }

    /// What resuming the coroutine gives.
    #[must_use]
    pub const fn progress(&self) -> &CoroutineProgress { &self.progress }

    /// The signature of `coroutine_resume`.
    #[must_use]
    pub const fn resume_signature(&self) -> &FunctionSignature { &self.resume_signature }
}

/// Derives the interface of a concrete coroutine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<CoroutineSignature>)]
#[extend(name = get_coroutine_signature, by_val)]
pub struct CoroutineSignatureKey {
    /// The coroutine whose interface is requested.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn coroutine_signature_executor(
    key: &CoroutineSignatureKey,
    engine: &TrackedEngine,
) -> Interned<CoroutineSignature> {
    let solver = Solver::without_givens(engine.clone()).await;
    let view = key.instance.view();
    let signature_id = view.signature_id(engine).await;
    let substitution = view.create_subst(engine).await;

    // A closure runs the signature of `core.Def.call`, whose return type and
    // effect are projections that its dictionary tells.
    let row =
        EffectRow::new(&solver, &engine.get_effect_row(signature_id).await, &substitution).await;
    let return_type =
        lower_type(&solver, &engine.get_return_type(signature_id).await, &substitution).await;
    let progress = CoroutineProgress::new(row.clone(), return_type);

    // `coroutine_resume(frame, args)` returns the progress.
    let frame =
        engine.intern(MonoType::Aggregate(AggregateType::CoroutineFrame(key.instance.clone())));
    let frame = MonoType::new_pointer(frame, PointerMutability::Mut, engine);
    let args = engine.intern(MonoType::Aggregate(AggregateType::CoroutineResume(
        CoroutineResume::new(row.clone()),
    )));

    let progress_type =
        engine.intern(MonoType::Aggregate(AggregateType::CoroutineProgress(progress.clone())));

    let resume_signature = FunctionSignature::builder()
        .parameter_types(engine.intern_unsized([frame, args]))
        .return_type(ReturnType::Value(progress_type))
        .engine(engine)
        .build();

    engine.intern(CoroutineSignature { row, progress, resume_signature })
}

#[distributed_slice(RAY_PROGRAM)]
static COROUTINE_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<CoroutineSignatureKey, CoroutineSignatureExecutor>();

#[distributed_slice(RAY_PROGRAM)]
static CALLBACK_DEF_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<CallbackDefSignatureKey, CallbackDefSignatureExecutor>();

#[distributed_slice(RAY_PROGRAM)]
static CALLBACK_FUNCTION_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<CallbackFunctionSignatureKey, CallbackFunctionSignatureExecutor>();
