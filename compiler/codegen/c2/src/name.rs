use std::fmt;

use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
};
use rayc_mono_ir::{
    MonoDefInstance,
    function::{MonoFunctionID, MonoFunctionKind},
    ty::AggregateType,
};
use rayc_symbol::GlobalSymbolID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct AggregateID(u128);

impl AggregateID {
    pub(super) fn for_type(aggregate: &AggregateType) -> Self {
        let domain = match aggregate {
            AggregateType::EffectHandler(_) => "rayc_c2::EffectHandlerLayout:v1",
            AggregateType::Tuple(_) => "rayc_c2::TupleLayout:v1",
            AggregateType::Environment(_) => "rayc_c2::EnvironmentLayout:v1",
        };
        Self(stable_codegen_id(domain, aggregate))
    }

    const fn base62(self) -> Base62 { Base62(self.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct DefinitionID(u128);

impl DefinitionID {
    fn for_symbol(symbol: GlobalSymbolID) -> Self {
        Self(stable_codegen_id("rayc_c2::DefinitionSymbol:v1", &symbol))
    }

    fn for_substitution(instance: &MonoDefInstance) -> Self {
        Self(stable_codegen_id("rayc_c2::DefinitionSubstitution:v1", instance.substitution()))
    }

    const fn base62(self) -> Base62 { Base62(self.0) }
}

pub(super) fn aggregate_name(aggregate: &AggregateType) -> String {
    let category = match aggregate {
        AggregateType::EffectHandler(_) => "EffectHandler",
        AggregateType::Tuple(_) => "Tuple",
        AggregateType::Environment(_) => "Environment",
    };
    format!("Ray{category}_{}", AggregateID::for_type(aggregate).base62())
}

pub(super) fn aggregate_typedef_name(aggregate: &AggregateType) -> String {
    format!("{}_t", aggregate_name(aggregate))
}

pub(super) fn definition_name(instance: &MonoDefInstance) -> String {
    let symbol = DefinitionID::for_symbol(instance.def_id()).base62();
    let substitution = DefinitionID::for_substitution(instance).base62();
    format!("ray_def_{symbol}_{substitution}")
}

pub(super) fn function_name(
    instance: &MonoDefInstance,
    function_id: MonoFunctionID,
    kind: MonoFunctionKind,
    root_id: MonoFunctionID,
) -> String {
    let definition = definition_name(instance);
    if function_id == root_id {
        return definition;
    }

    let category = match kind {
        MonoFunctionKind::Def => {
            panic!("compiler-internal invariant violation: nested function has def kind")
        }
        MonoFunctionKind::Lambda => "lambda",
        MonoFunctionKind::Thunk => "thunk",
        MonoFunctionKind::OperationHandler => "handler",
    };
    format!("{definition}_{category}_{:X}", function_id.index())
}

pub(super) fn local_name(index: u64) -> String { format!("ray_local_{index:X}") }

pub(super) fn block_name(index: u64) -> String { format!("ray_block_{index:X}") }

pub(super) fn tuple_field_name(index: u32) -> String { format!("elem{index:X}") }

pub(super) fn environment_field_name(index: u32) -> String { format!("capture{index:X}") }

pub(super) fn operation_environment_field_name(operation: GlobalSymbolID) -> String {
    format!("operation_{}_environment", operation_id(operation))
}

pub(super) fn operation_function_field_name(operation: GlobalSymbolID) -> String {
    format!("operation_{}_function", operation_id(operation))
}

fn operation_id(operation: GlobalSymbolID) -> Base62 {
    Base62(stable_codegen_id("rayc_c2::EffectOperation:v1", &operation))
}

fn stable_codegen_id<T: StableHash + ?Sized>(domain: &'static str, value: &T) -> u128 {
    let mut hasher = Sip128Hasher::default();
    domain.stable_hash(&mut hasher);
    value.stable_hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Base62(u128);

impl fmt::Display for Base62 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const ALPHABET: &[u8; 62] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const MAX_ENCODED_LENGTH: usize = 22;

        let mut value = self.0;
        let mut encoded = [0; MAX_ENCODED_LENGTH];
        let mut start = encoded.len();
        loop {
            start -= 1;
            encoded[start] = ALPHABET[(value % 62) as usize];
            value /= 62;
            if value == 0 {
                break;
            }
        }

        let encoded = str::from_utf8(&encoded[start..]).map_err(|_| fmt::Error)?;
        formatter.write_str(encoded)
    }
}

pub(super) fn closure_name(instance: &rayc_mono_ir::MonoClosureInstance) -> String {
    format!("ray_nominal_{}", Base62(stable_codegen_id("rayc_c2::NominalClosure:v1", instance)))
}

pub(super) fn ir_function_name(ir: &rayc_mono_ir::MonoIR, id: MonoFunctionID) -> String {
    ir.closure_instance(id).map_or_else(
        || function_name(ir.instance(), id, ir.get_function(id).kind(), ir.root_id()),
        |closure| closure_name(&closure),
    )
}
