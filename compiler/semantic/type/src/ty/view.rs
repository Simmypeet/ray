use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    poly_var::GlobalPolyVarID,
    ty::{
        Integer, Mutability, Primitive, Ty, application,
        application::{AdtView, EnumView, InstanceView, StructView, View as ApplicationView},
        inference::{GenInfer, Inference},
        lifetime::{ExternalRegionID, Lifetime, RegionID},
    },
};

impl Ty {
    #[must_use]
    pub fn unwrap_as_application_view(&self) -> ApplicationView<'_> {
        let Self::Application(ty_application) = self else {
            panic!("Expected Ty::Application, found {self:?}");
        };

        ty_application.view()
    }

    #[must_use]
    pub fn as_tuple_view(&self) -> Option<application::TupleView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Tuple(tuple_view) = ty_application.view() else {
            return None;
        };

        Some(tuple_view)
    }

    #[must_use]
    pub fn as_instance_view(&self) -> Option<InstanceView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Instance(instance_view) = ty_application.view() else {
            return None;
        };

        Some(instance_view)
    }

    #[must_use]
    pub fn as_instance_associated_view(&self) -> Option<application::InstanceAssociatedView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::InstanceAssociated(view) = ty_application.view() else {
            return None;
        };

        Some(view)
    }

    /// Returns the type arguments of an application, or nothing for any
    /// other type.
    pub fn interned_arguments(ty: &Interned<Self>) -> impl Iterator<Item = &Interned<Self>> {
        let arguments = match &**ty {
            Self::Application(application) => Some(application.interned_iter()),
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => None,
        };
        arguments.into_iter().flatten()
    }

    /// Returns whether this lifetime takes part in named-lifetime outlives
    /// checks. Erased lifetimes are checked on the IR instead, and errors
    /// were already reported.
    #[must_use]
    pub fn is_checked_lifetime(&self) -> bool {
        *self != Self::Lifetime(Lifetime::Erased) && !self.contains_error()
    }

    #[must_use]
    pub fn as_struct_view(&self) -> Option<StructView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Struct(struct_view) = ty_application.view() else {
            return None;
        };

        Some(struct_view)
    }

    /// Returns the struct or enum this type instantiates, if it is one.
    #[must_use]
    pub fn as_adt_id(&self) -> Option<GlobalSymbolID> {
        self.as_adt_view().map(|adt| adt.symbol_id())
    }

    /// Returns the view of the struct or enum this type instantiates, if it
    /// is one.
    #[must_use]
    pub fn as_adt_view(&self) -> Option<AdtView<'_>> {
        self.as_struct_view()
            .map(AdtView::Struct)
            .or_else(|| self.as_enum_view().map(AdtView::Enum))
    }

    #[must_use]
    pub fn as_enum_view(&self) -> Option<EnumView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Enum(enum_view) = ty_application.view() else {
            return None;
        };

        Some(enum_view)
    }

    #[must_use]
    pub const fn as_poly_var(&self) -> Option<&GlobalPolyVarID> {
        if let Self::PolyVar(poly_var) = self { Some(poly_var) } else { None }
    }

    /// Returns the region variable this type is, if it is one; see
    /// [`Lifetime::Region`].
    #[must_use]
    pub const fn as_region(&self) -> Option<RegionID> {
        if let Self::Lifetime(Lifetime::Region(region)) = self { Some(*region) } else { None }
    }

    /// Returns whether this type is the `'static` lifetime.
    #[must_use]
    pub const fn is_static_lifetime(&self) -> bool {
        matches!(self, Self::Lifetime(Lifetime::Static))
    }

    /// Returns whether this type is an external lifetime; see
    /// [`Lifetime::External`].
    #[must_use]
    pub const fn is_external_lifetime(&self) -> bool {
        matches!(self, Self::Lifetime(Lifetime::External(_)))
    }

    /// Returns the external lifetime this type is, if it is one; see
    /// [`Lifetime::External`].
    #[must_use]
    pub const fn as_external_region(&self) -> Option<ExternalRegionID> {
        if let Self::Lifetime(Lifetime::External(external)) = self { Some(*external) } else { None }
    }

    #[must_use]
    pub fn open_closed_row(
        ty: &Interned<Self>,
        infer_gen: &mut impl GenInfer,
        engine: &TrackedEngine,
    ) -> Option<Interned<Self>> {
        if let Self::EffectRow(row) = ty.as_ref() {
            row.open_closed_row(infer_gen, engine).map(|x| engine.intern(Self::EffectRow(x)))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_inference(&self) -> Option<&Inference> {
        if let Self::Inference(inference) = self { Some(inference) } else { None }
    }

    #[must_use]
    pub fn as_pointer_mutability(&self) -> Option<Mutability> {
        if let Self::Application(application) = self
            && let ApplicationView::Pointer(pointer) = application.view()
        {
            return Some(pointer.mutability());
        }

        None
    }

    #[must_use]
    pub fn as_reference_view(&self) -> Option<application::ReferenceView<'_>> {
        if let Self::Application(application) = self
            && let ApplicationView::Reference(reference) = application.view()
        {
            return Some(reference);
        }
        None
    }

    /// Returns what a dereference of this type reaches, when it is a raw
    /// pointer or a reference.
    #[must_use]
    pub fn as_dereferenceable(&self) -> Option<Dereferenceable<'_>> {
        let Self::Application(application) = self else {
            return None;
        };
        match application.view() {
            ApplicationView::Pointer(pointer) => Some(Dereferenceable {
                pointee: pointer.pointee(),
                mutability: pointer.mutability(),
                is_raw: true,
            }),
            ApplicationView::Reference(reference) => Some(Dereferenceable {
                pointee: reference.pointee(),
                mutability: reference.mutability(),
                is_raw: false,
            }),
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Enum(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::DefCoroutine(_)
            | ApplicationView::TraitDefCoroutine(_)
            | ApplicationView::ClosureCoroutine(_)
            | ApplicationView::ContinuationDefInstance(_)
            | ApplicationView::CoroutineInstance(_)
            | ApplicationView::CoroutineDropInstance(_)
            | ApplicationView::GeneratedAdtDropInstance(_)
            | ApplicationView::Error => None,
        }
    }

    #[must_use]
    pub fn as_pointee_of_pointer(&self) -> Option<&Interned<Self>> {
        if let Self::Application(application) = self
            && let ApplicationView::Pointer(pointer) = application.view()
        {
            return Some(pointer.pointee());
        }
        None
    }

    #[must_use]
    pub fn is_opaque_projection(&self) -> Option<bool> {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::InstanceAssociated(associated) => Some(matches!(
                    &**associated.instance(),
                    Self::PolyVar(_) | Self::SelfInstance(_)
                )),
                ApplicationView::Error => None,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::Enum(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::DefCoroutine(_)
                | ApplicationView::TraitDefCoroutine(_)
                | ApplicationView::ClosureCoroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)
                | ApplicationView::Instance(_) => Some(false),
            },
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => Some(false),
        }
    }

    #[must_use]
    pub fn unwrap_as_closure_view(&self) -> application::ClosureView<'_> {
        self.as_closure_view().expect("expected a closure type")
    }

    #[must_use]
    pub fn as_closure_view(&self) -> Option<application::ClosureView<'_>> {
        let Self::Application(application) = self else {
            return None;
        };

        let ApplicationView::Closure(closure_view) = application.view() else {
            return None;
        };

        Some(closure_view)
    }

    #[must_use]
    pub fn as_def_coroutine_view(&self) -> Option<application::DefCoroutineView<'_>> {
        let Self::Application(application) = self else {
            return None;
        };

        let ApplicationView::DefCoroutine(coroutine_view) = application.view() else {
            return None;
        };

        Some(coroutine_view)
    }

    /// Returns the coroutine type this type is, if it suspends a known
    /// function rather than a call through an instance.
    #[must_use]
    pub fn as_concrete_coroutine_view(&self) -> Option<application::ConcreteCoroutineView<'_>> {
        match self.as_coroutine_view()? {
            application::CoroutineView::Def(view) => {
                Some(application::ConcreteCoroutineView::Def(view))
            }
            application::CoroutineView::Closure(view) => {
                Some(application::ConcreteCoroutineView::Closure(view))
            }
            application::CoroutineView::TraitDef(_) => None,
        }
    }

    /// Returns the coroutine type this type is, whether it suspends a known
    /// function or a call through an instance.
    #[must_use]
    pub fn as_coroutine_view(&self) -> Option<application::CoroutineView<'_>> {
        let Self::Application(application) = self else {
            return None;
        };

        match application.view() {
            ApplicationView::DefCoroutine(view) => Some(application::CoroutineView::Def(view)),
            ApplicationView::TraitDefCoroutine(view) => {
                Some(application::CoroutineView::TraitDef(view))
            }
            ApplicationView::ClosureCoroutine(view) => {
                Some(application::CoroutineView::Closure(view))
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Enum(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::GeneratedAdtDropInstance(_)
            | ApplicationView::CoroutineInstance(_)
            | ApplicationView::ContinuationDefInstance(_)
            | ApplicationView::CoroutineDropInstance(_)
            | ApplicationView::Error => None,
        }
    }

    #[must_use]
    pub fn as_trait_def_coroutine_view(&self) -> Option<application::TraitDefCoroutineView<'_>> {
        let Self::Application(application) = self else {
            return None;
        };

        let ApplicationView::TraitDefCoroutine(view) = application.view() else {
            return None;
        };

        Some(view)
    }

    /// Returns the coroutine type of either kind this type is, or is a `&mut`
    /// to.
    ///
    /// Both have a built-in `Coroutine` instance: a continuation of a
    /// coroutine pinned to the stack only holds a `&mut` to it.
    #[must_use]
    pub fn as_coroutine_behind_reference(&self) -> Option<application::CoroutineView<'_>> {
        match self.as_reference_view() {
            Some(reference) if reference.mutability() == Mutability::Mutable => {
                reference.pointee().as_coroutine_view()
            }
            Some(_) => None,
            None => self.as_coroutine_view(),
        }
    }

    #[must_use]
    pub fn is_c_abi_value_type(&self) -> bool {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::Primitive(_) => true,
                ApplicationView::Pointer(pointer) => pointer.pointee().is_c_abi_value_type(),
                // A reference reaches C through a coercion to a raw pointer.
                ApplicationView::Reference(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Struct(_)
                | ApplicationView::Enum(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Instance(_)
                | ApplicationView::Closure(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::DefCoroutine(_)
                | ApplicationView::TraitDefCoroutine(_)
                | ApplicationView::ClosureCoroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)
                | ApplicationView::Error => false,
            },
            Self::EffectRow(_)
            | Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        }
    }

    /// Returns whether this is an effect row that is syntactically empty; see
    /// [`EffectRow::is_empty`].
    ///
    /// Anything that is not a row is not empty: a row variable may be
    /// instantiated with an effect.
    #[must_use]
    pub fn is_empty_effect_row(&self) -> bool {
        match self {
            Self::EffectRow(row) => row.is_empty(),
            Self::Application(_)
            | Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        }
    }

    #[must_use]
    pub fn is_unit_type(&self) -> bool {
        matches!(self, Self::Application(application) if matches!(application.view(), ApplicationView::Tuple(tuple) if tuple.args().is_empty()))
    }

    /// Returns whether this is a projection through an instance; see
    /// [`Application::is_projection`].
    #[must_use]
    pub const fn is_projection(&self) -> bool {
        matches!(self, Self::Application(application) if application.is_projection())
    }

    #[must_use]
    pub fn is_instance_associated(&self) -> bool {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::InstanceAssociated(_) => true,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::Enum(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Instance(_)
                | ApplicationView::Closure(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::DefCoroutine(_)
                | ApplicationView::TraitDefCoroutine(_)
                | ApplicationView::ClosureCoroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)
                | ApplicationView::Error => false,
            },
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => false,
        }
    }

    /// Returns the primitive type this type is, if any.
    #[must_use]
    pub fn as_primitive(&self) -> Option<Primitive> {
        let Self::Application(application) = self else {
            return None;
        };
        let ApplicationView::Primitive(primitive) = application.view() else {
            return None;
        };

        Some(primitive)
    }

    #[must_use]
    pub fn is_int32(&self) -> bool {
        matches!(self, Self::Application(application) if matches!(application.view(), ApplicationView::Primitive(Primitive::Integer(Integer::Int32))))
    }
}

/// What a dereference of a raw pointer or a reference reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dereferenceable<'x> {
    pointee: &'x Interned<Ty>,
    mutability: Mutability,
    is_raw: bool,
}

impl<'x> Dereferenceable<'x> {
    #[must_use]
    pub const fn pointee(&self) -> &'x Interned<Ty> { self.pointee }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }

    /// Returns whether this is a raw pointer rather than a reference.
    #[must_use]
    pub const fn is_raw(&self) -> bool { self.is_raw }
}
