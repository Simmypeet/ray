//! The layouts of nominal aggregates: what fields a value of each type has, and
//! the tags that tell its variants apart.
//!
//! A nominal aggregate type only names what it is, so lowering a type never
//! lowers its fields. The queries here derive those fields on demand.
//!
//! The fields of a `coroutine_frame_t` come from the body of the suspended
//! function, so [`BuildCoroutineFrame`] reads its semantic IR.

use std::collections::BTreeMap;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_ir::ir_function::{IRFunctionID, IRFunctionMap};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map,
    struct_body::{FieldID, get_struct_body},
};
use rayc_solver::Solver;
use rayc_symbol::{GlobalSymbolID, member::get_members};
use rayc_type::{
    signature::get_return_type,
    ty::application::{ClosureID, ConcreteCoroutineView},
};

use crate::{
    instance::{MonoDefInstance, MonoEffectInstance, MonoStructInstance},
    operand::Constant,
    ty::{FunctionSignature, MonoType, lower_type},
};

pub mod coroutine;

/// The unsigned integer type of a tag, which tells the variant of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    Uint8,
    Uint16,
    Uint32,
    Uint64,
}

impl TagType {
    /// The narrowest type that can tell `count` variants apart, whose tags are
    /// `0..count`.
    #[must_use]
    pub fn for_variant_count(count: usize) -> Self {
        let largest = count.saturating_sub(1);
        if u8::try_from(largest).is_ok() {
            Self::Uint8
        } else if u16::try_from(largest).is_ok() {
            Self::Uint16
        } else if u32::try_from(largest).is_ok() {
            Self::Uint32
        } else {
            Self::Uint64
        }
    }

    /// The runtime representation of the tag.
    #[must_use]
    pub const fn mono_type(self) -> MonoType {
        match self {
            Self::Uint8 => MonoType::Uint8,
            Self::Uint16 => MonoType::Uint16,
            Self::Uint32 => MonoType::Uint32,
            Self::Uint64 => MonoType::Uint64,
        }
    }

    /// The tag `value` as a constant of this type.
    ///
    /// # Panics
    ///
    /// Panics if `value` does not fit, so a tag is never silently truncated.
    #[must_use]
    pub fn constant(self, value: u64) -> Constant {
        const FITS: &str = "a tag should fit the type chosen for its variant count";
        match self {
            Self::Uint8 => Constant::Uint8(value.try_into().expect(FITS)),
            Self::Uint16 => Constant::Uint16(value.try_into().expect(FITS)),
            Self::Uint32 => Constant::Uint32(value.try_into().expect(FITS)),
            Self::Uint64 => Constant::Uint64(value),
        }
    }
}

/// The fields of one concrete struct.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct StructLayout {
    instance: MonoStructInstance,
    fields: BTreeMap<FieldID, Interned<MonoType>>,
}

impl StructLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoStructInstance { &self.instance }

    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<FieldID, Interned<MonoType>> { &self.fields }
}

/// One callback slot in a concrete effect-handler record.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectOperation {
    operation_id: GlobalSymbolID,
    signature: FunctionSignature,
}

impl EffectOperation {
    #[must_use]
    pub const fn new(operation_id: GlobalSymbolID, signature: FunctionSignature) -> Self {
        Self { operation_id, signature }
    }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }
}

/// The operation slots of one nominal, concrete effect-handler record.
///
/// C code generation can represent this as a struct with one closure field per
/// operation. Each closure consists of a callback address with the operation's
/// [`FunctionSignature`] and an opaque environment pointer. There is no
/// continuation or resumption slot: invoking an operation is an ordinary
/// callback call that returns to its caller.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct HandlerLayout {
    instance: MonoEffectInstance,
    operations: Vec<EffectOperation>,
}

impl HandlerLayout {
    #[must_use]
    pub const fn new(instance: MonoEffectInstance, operations: Vec<EffectOperation>) -> Self {
        Self { instance, operations }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoEffectInstance { &self.instance }

    #[must_use]
    pub fn operations(&self) -> &[EffectOperation] { &self.operations }
}

/// Derives the callback slots for one concrete effect instantiation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<HandlerLayout>)]
#[extend(name = build_handler_layout, by_val)]
pub struct BuildHandlerLayout {
    /// The concrete effect whose operation signatures determine the layout.
    pub instance: MonoEffectInstance,
}

#[executor(config = Config)]
async fn build_handler_layout_executor(
    key: &BuildHandlerLayout,
    engine: &TrackedEngine,
) -> Interned<HandlerLayout> {
    let instance = &key.instance;
    let solver = Solver::without_givens(engine.clone()).await;
    let members = engine.get_members(instance.effect_id()).await;
    let mut operations = members
        .namable_members()
        .map(|operation| instance.effect_id().target_id.make_global(operation))
        .collect::<Vec<_>>();
    operations.sort_unstable();

    let mut lowered_operations = Vec::with_capacity(operations.len());
    for operation_id in operations {
        let parameters = engine.get_parameter_map(operation_id).await;
        let mut parameter_types = vec![MonoType::new_opaque_pointer(engine)];
        for (_, parameter) in parameters.iter() {
            parameter_types
                .push(lower_type(&solver, parameter.ty(), instance.substitution()).await);
        }
        let return_type = engine.get_return_type(operation_id).await;
        let return_type = lower_type(&solver, &return_type, instance.substitution()).await;
        lowered_operations.push(EffectOperation::new(
            operation_id,
            MonoType::new_function_signature(parameter_types, return_type, engine),
        ));
    }
    engine.intern(HandlerLayout::new(instance.clone(), lowered_operations))
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_HANDLER_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildHandlerLayout, BuildHandlerLayoutExecutor>();

/// Derives the fields of one concrete struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<StructLayout>)]
#[extend(name = build_struct_layout, by_val)]
pub struct BuildStructLayout {
    /// The concrete struct whose declared fields determine the layout.
    pub instance: MonoStructInstance,
}

#[executor(config = Config)]
async fn build_struct_layout_executor(
    key: &BuildStructLayout,
    engine: &TrackedEngine,
) -> Interned<StructLayout> {
    let instance = &key.instance;
    let solver = Solver::without_givens(engine.clone()).await;
    let struct_body = engine.get_struct_body(instance.struct_id()).await;

    // Lower each field's type under the concrete type argument substitution.
    let mut fields = BTreeMap::new();
    for (field_id, field) in struct_body.iter() {
        fields.insert(field_id, lower_type(&solver, field.ty(), instance.substitution()).await);
    }

    engine.intern(StructLayout { instance: instance.clone(), fields })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_STRUCT_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildStructLayout, BuildStructLayoutExecutor>();

/// The function a coroutine suspends, in the IR of its owning definition.
#[derive(Debug)]
pub struct CoroutineBody {
    owner: MonoDefInstance,
    closure: Option<ClosureID>,
}

impl CoroutineBody {
    pub async fn of(view: ConcreteCoroutineView<'_>, solver: &Solver) -> Self {
        let engine = solver.engine();
        match view {
            ConcreteCoroutineView::Def(coroutine) => {
                let substitution = coroutine.create_subst(engine).await;
                let owner = MonoDefInstance::new(coroutine.def_id(), substitution, solver).await;
                Self { owner, closure: None }
            }
            ConcreteCoroutineView::Closure(coroutine) => {
                let closure = coroutine.closure_view();
                let substitution = closure.create_subst(engine).await;
                let owner = MonoDefInstance::new(closure.owner_id(), substitution, solver).await;
                Self { owner, closure: Some(closure.local_closure_id()) }
            }
        }
    }

    #[must_use]
    pub fn into_owner(self) -> MonoDefInstance { self.owner }

    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.owner.def_id() }

    #[must_use]
    pub fn function_id(&self, source: &IRFunctionMap) -> IRFunctionID {
        self.closure
            .map_or_else(|| source.root_id(), |closure| source.get_function_id_of_closure(closure))
    }
}

#[cfg(test)]
mod tests {
    use super::TagType;

    /// Input: variant counts at each boundary of the unsigned integer types.
    /// Premise: tags are `0..count`, so a type fits one more variant than its
    /// largest value. Output: the narrowest type that holds the largest tag.
    #[test]
    fn tag_type_is_the_narrowest_that_holds_the_largest_tag() {
        assert_eq!(TagType::for_variant_count(0), TagType::Uint8);
        assert_eq!(TagType::for_variant_count(256), TagType::Uint8);
        assert_eq!(TagType::for_variant_count(257), TagType::Uint16);
        assert_eq!(TagType::for_variant_count(1 << 16), TagType::Uint16);
        assert_eq!(TagType::for_variant_count((1 << 16) + 1), TagType::Uint32);
        assert_eq!(TagType::for_variant_count(1 << 32), TagType::Uint32);
        assert_eq!(TagType::for_variant_count((1 << 32) + 1), TagType::Uint64);
    }
}
