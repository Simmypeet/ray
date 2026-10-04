//! Typing of places, which the printer needs to choose the C type a value
//! is expected to have.

use rayc_mono_ir::{
    place::{Place, Projection},
    ty::{AggregateType, FunctionSignature, MonoType},
};
use rayc_symbol::GlobalSymbolID;

use crate::print::FunctionPrinter;

/// The type of a place, which is either a value type or the callback slot
/// of an effect-handler record (a function pointer with no interned
/// [`MonoType`]).
#[derive(Debug, Clone, Copy)]
pub(super) enum PlaceType<'a> {
    Value(&'a MonoType),
    OperationFunction(&'a FunctionSignature),
}

impl<'a> PlaceType<'a> {
    pub(super) const fn as_value(self) -> Option<&'a MonoType> {
        match self {
            Self::Value(ty) => Some(ty),
            Self::OperationFunction(_) => None,
        }
    }

    const fn expect_value(self) -> &'a MonoType {
        self.as_value().expect("a callback slot has no projectable members")
    }

    pub(super) fn expect_function_signature(self) -> &'a FunctionSignature {
        match self {
            Self::Value(MonoType::FunctionPointer(signature))
            | Self::OperationFunction(signature) => signature,
            Self::Value(_) => panic!("an indirect call callee should have function-pointer type"),
        }
    }
}

impl<'a> FunctionPrinter<'a> {
    /// The type of the value stored at `place`.
    pub(super) fn place_type(&self, place: &Place) -> PlaceType<'a> {
        let local = self.function.get_local(place.local());
        place.projections().iter().fold(PlaceType::Value(local.ty()), |ty, projection| {
            self.project(ty.expect_value(), *projection)
        })
    }

    fn project(&self, ty: &'a MonoType, projection: Projection) -> PlaceType<'a> {
        match (projection, ty) {
            (Projection::Dereference, MonoType::Pointer(pointer)) => {
                PlaceType::Value(pointer.pointee())
            }
            (
                Projection::EnvironmentFieldIndex(index),
                MonoType::Aggregate(AggregateType::Environment(environment)),
            ) => PlaceType::Value(
                environment
                    .captures()
                    .get(index.index() as usize)
                    .expect("environment projection index should be in bounds"),
            ),
            (
                Projection::TupleFieldIndex(index),
                MonoType::Aggregate(AggregateType::Tuple(tuple)),
            ) => PlaceType::Value(
                tuple
                    .fields()
                    .get(index.index() as usize)
                    .expect("tuple projection index should be in bounds"),
            ),
            (
                Projection::StructFieldIndex(field),
                MonoType::Aggregate(AggregateType::Struct(st)),
            ) => PlaceType::Value(
                st.fields().get(&field).expect("struct projection field should be in bounds"),
            ),
            (Projection::OperationRecordEnvironmentField(operation), _) => PlaceType::Value(
                self.operation_signature(ty, operation)
                    .parameter_types()
                    .first()
                    .expect("operation signature should have an environment parameter"),
            ),
            (Projection::OperationRecordFunctionPointerField(operation), _) => {
                PlaceType::OperationFunction(self.operation_signature(ty, operation))
            }
            (
                Projection::Dereference
                | Projection::EnvironmentFieldIndex(_)
                | Projection::TupleFieldIndex(_)
                | Projection::StructFieldIndex(_),
                _,
            ) => panic!("MonoIR projection {projection:?} does not apply to type {ty:?}"),
        }
    }

    /// The signature of `operation` in the effect-handler record `ty`.
    fn operation_signature(
        &self,
        ty: &'a MonoType,
        operation: GlobalSymbolID,
    ) -> &'a FunctionSignature {
        let MonoType::Aggregate(AggregateType::EffectHandler(handler)) = ty else {
            panic!("operation projection requires an effect-handler aggregate")
        };
        self.aggregates
            .handler_layout(handler.mono_effect_instance())
            .operations()
            .iter()
            .find(|candidate| candidate.operation_id() == operation)
            .expect("operation is absent from its concrete handler layout")
            .signature()
    }
}
