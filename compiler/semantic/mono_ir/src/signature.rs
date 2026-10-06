//! Calling signatures of concrete definitions, derived from their
//! declarations alone.
//!
//! A caller only needs a callee's interface, so the signature is a query of
//! its own rather than something read off the callee's lowered body. Callers
//! therefore depend on the declaration, not the implementation, of what they
//! call.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_solver::Solver;
use rayc_symbol::{
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};

use crate::{
    MonoDefInstance, MonoEffectInstance,
    ty::{FunctionSignature, MonoType, ReturnType, lower_effects, lower_type},
};

/// The calling convention of one concrete definition instance.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct DefSignature {
    signature: FunctionSignature,
    effects: Vec<MonoEffectInstance>,
}

impl DefSignature {
    /// The C-level signature, including one trailing handler pointer per
    /// effect.
    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }

    /// The effects whose handlers a caller passes after the source
    /// arguments, in parameter order.
    #[must_use]
    pub fn effects(&self) -> &[MonoEffectInstance] { &self.effects }
}

/// Derives the calling signature of a concrete definition instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<DefSignature>)]
#[extend(name = get_def_signature, by_val)]
pub struct DefSignatureKey {
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
            | SymbolKind::Instance
            | SymbolKind::Marker
            | SymbolKind::MarkerImplementation
            | SymbolKind::Module
            | SymbolKind::Strut
            | SymbolKind::Trait
            | SymbolKind::TraitType
            | SymbolKind::InstanceType
            | SymbolKind::TraitDef => panic!("{kind:?} symbol has no calling signature"),
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
async fn def_signature_executor(
    key: &DefSignatureKey,
    engine: &TrackedEngine,
) -> Interned<DefSignature> {
    let def_id = key.instance.def_id();
    let substitution = key.instance.substitution();
    let kind = DefKind::of(engine.get_symbol_kind(def_id).await);
    let solver = Solver::without_givens(engine.clone()).await;

    // Source parameters, then one handler pointer per effect.
    let mut parameter_types = Vec::new();
    for (_, parameter) in engine.get_parameter_map(def_id).await.iter() {
        parameter_types.push(lower_type(&solver, parameter.ty(), substitution).await);
    }
    let effects = if kind.receives_handlers() {
        lower_effects(&solver, &engine.get_effect_row(def_id).await, substitution).await
    } else {
        Vec::new()
    };
    parameter_types
        .extend(effects.iter().map(|effect| MonoType::new_handler_pointer(effect.clone(), engine)));

    let return_type =
        lower_type(&solver, &engine.get_return_type(def_id).await, substitution).await;
    let return_type = if kind.returns_unit_as_void() && return_type.is_unit() {
        ReturnType::Void
    } else {
        ReturnType::Value(return_type)
    };

    let parameter_types = engine.intern_unsized(parameter_types);
    let signature = if kind.may_be_variadic() && engine.is_variadic_def(def_id).await {
        FunctionSignature::new_variadic(parameter_types, return_type)
    } else {
        FunctionSignature::new(parameter_types, return_type)
    };
    engine.intern(DefSignature { signature, effects })
}

#[distributed_slice(RAY_PROGRAM)]
static DEF_SIGNATURE_EXECUTOR: Registration<Config> =
    Registration::new::<DefSignatureKey, DefSignatureExecutor>();
