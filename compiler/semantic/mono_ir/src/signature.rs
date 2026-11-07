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
    ir_function::{IRContext, IRFunctionMap},
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};
use rayc_type::{
    signature::{get_effect_row, get_return_type},
    subst::Subst,
};

use crate::{
    MonoBodyInstance, MonoCoroutineInstance, MonoEnvironmentInstance,
    ty::{
        AggregateType, CoroutineProgress, CoroutineResume, EffectRow, EnvironmentSlot,
        FunctionSignature, MonoType, PointerMutability, ReturnType, lower_type,
    },
};

/// Derives the callback-style signature of a function in the IR of a concrete
/// definition instantiation: the definition itself, the body of one of its
/// closures, the body of a `run ... with`, or the clause of one.
///
/// The signature of the definition function is its declared signature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(FunctionSignature)]
#[extend(name = get_callback_signature, by_val)]
pub struct CallbackSignatureKey {
    /// The function whose interface is requested.
    pub instance: MonoBodyInstance,
}

/// Derives the C signature of an `extern def` from its declaration, since it
/// has no IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(FunctionSignature)]
#[extend(name = get_extern_def_signature, by_val)]
pub struct ExternDefSignatureKey {
    /// The `extern def` whose interface is requested.
    pub def_id: GlobalSymbolID,
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
async fn callback_signature_executor(
    key: &CallbackSignatureKey,
    engine: &TrackedEngine,
) -> FunctionSignature {
    let instance = &key.instance;
    let source = engine.get_ir(instance.def_id()).await;

    // The definition implements exactly its declared signature, which is what
    // its callers spell.
    if instance.ir_function_id() == source.root_id() {
        declared_signature(instance.def_id(), instance.substitution(), engine).await
    } else {
        nested_function_signature(instance, &source, engine).await
    }
}

#[executor(config = Config)]
async fn extern_def_signature_executor(
    key: &ExternDefSignatureKey,
    engine: &TrackedEngine,
) -> FunctionSignature {
    // An `extern def` cannot be polymorphic, so nothing needs substituting.
    declared_signature(key.def_id, &Subst::new_empty(), engine).await
}

/// The signature that the definition `def_id` declares, instantiated with
/// `substitution`.
async fn declared_signature(
    def_id: GlobalSymbolID,
    substitution: &Subst,
    engine: &TrackedEngine,
) -> FunctionSignature {
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

    let return_type =
        lower_type(&solver, &engine.get_return_type(def_id).await, substitution).await;
    let return_type = if kind.returns_unit_as_void() && return_type.is_unit() {
        ReturnType::Void
    } else {
        ReturnType::Value(return_type)
    };

    let parameter_types = engine.intern_unsized(parameter_types);
    let variadic = kind.may_be_variadic() && engine.is_variadic_def(def_id).await;

    FunctionSignature::builder()
        .parameter_types(parameter_types)
        .return_type(return_type)
        .variadic(variadic)
        .handler_record_slots(effects)
        .engine(engine)
        .build()
}

/// The signature of a function of `source`, the IR of `instance`, other than
/// the definition function, which receives an environment.
async fn nested_function_signature(
    instance: &MonoBodyInstance,
    source: &IRFunctionMap,
    engine: &TrackedEngine,
) -> FunctionSignature {
    let substitution = instance.substitution();
    let function_id = instance.ir_function_id();
    let function = source.get_function(function_id);
    let solver = Solver::without_givens(engine.clone()).await;

    // What the kind of the function decides: its source parameters, how it
    // receives its environment, and whether it receives handler records.
    let (parameters, environment_slot, receives_handler_records) = match function.context() {
        IRContext::Def => panic!("the definition function takes its declared signature"),

        // A closure body receives its environment by value, as callers of the
        // closure's `Def.call` pass it.
        IRContext::Closure(closure) => {
            let capture_map = source.get_capture_map_id(function_id);
            let environment = MonoEnvironmentInstance::new(instance, capture_map);
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

    // Lower the source parameters, the result and the effects in the
    // definition's substitution.
    let mut parameter_types = Vec::with_capacity(parameters.len());
    for parameter in &parameters {
        parameter_types.push(lower_type(&solver, parameter, substitution).await);
    }
    let return_type = function.context().return_ty(instance.def_id(), engine).await;
    let return_type = lower_type(&solver, &return_type, substitution).await;
    let handler_record_slots = if receives_handler_records {
        let effect = source.effect_of(function_id, engine).await;
        Some(EffectRow::new(&solver, &effect, substitution).await)
    } else {
        None
    };

    FunctionSignature::builder()
        .parameter_types(engine.intern_unsized(parameter_types))
        .return_type(ReturnType::Value(return_type))
        .environment_slot(environment_slot)
        .maybe_handler_record_slots(handler_record_slots)
        .engine(engine)
        .build()
}

/// The interface of a concrete coroutine, derived from the function it
/// suspends.
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
    let body = key.instance.body();
    let substitution = body.substitution();
    let source = engine.get_ir(body.def_id()).await;
    let function = source.get_function(body.ir_function_id());

    // The coroutine performs what the function performs, and returns what it
    // returns.
    let effect = source.effect_of(body.ir_function_id(), engine).await;
    let row = EffectRow::new(&solver, &effect, substitution).await;
    let return_type = function.context().return_ty(body.def_id(), engine).await;
    let return_type = lower_type(&solver, &return_type, substitution).await;
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
static CALLBACK_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<CallbackSignatureKey, CallbackSignatureExecutor>();

#[distributed_slice(RAY_PROGRAM)]
static EXTERN_DEF_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<ExternDefSignatureKey, ExternDefSignatureExecutor>();
