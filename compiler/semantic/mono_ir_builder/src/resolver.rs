//! Resolves dictionaries and callee signatures in one concrete owner.
//!
//! Both functions of source definitions and generated nominal Drop functions
//! call through dictionaries. The owner substitution is empty for a generated
//! Drop function because its dictionary key is already concrete.
//!
//! The resolver owns the single `Solver` of the function being lowered; every
//! normalization while lowering it goes through it rather than a fresh solver.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::frame_layout::FrameLayout;
use rayc_ir::{get_ir, ir_closure::CaptureID};
use rayc_mono_ir::{
    MonoBodyInstance, MonoCoroutineInstance, MonoEffectInstance, MonoFunctionInstance,
    MonoGeneratedAdtDropInstance, MonoStructInstance,
    layout::{
        StructLayout, build_struct_layout,
        coroutine::{
            EffectRowLayout, FrameFieldTypes, build_effect_row_layout, build_frame_field_types,
            build_frame_layout,
        },
    },
    signature::{
        CoroutineSignature, get_callback_signature, get_coroutine_signature,
        get_extern_def_signature,
    },
    ty::{EffectRow, FunctionSignature, MonoType, ReturnType, instantiate_effect, lower_type},
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_member_by_name,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    instance_member::get_instance_member,
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    ty::{
        Ty,
        application::{InstanceView, View as ApplicationView},
        args::Args,
    },
};

/// The concrete implementation selected by an instance call.
pub(crate) enum InstanceCallable {
    /// The definition function of a definition.
    Definition(MonoBodyInstance),
    /// The body of a closure, with its signature.
    Closure(MonoBodyInstance, FunctionSignature),
    TupleDrop(Vec<Interned<Ty>>),
    /// The `Drop` dictionary of each capture of a closure.
    ClosureDrop(Vec<(CaptureID, Interned<Ty>)>),
    NominalDrop(MonoGeneratedAdtDropInstance),
    /// `Def.call` of a continuation, which resumes this coroutine.
    Continuation(MonoCoroutineInstance),
    NoOp,
}

/// Instance-resolution state of the function being lowered.
pub(crate) struct Resolver {
    engine: TrackedEngine,
    solver: Solver,
    substitution: Subst,
}

impl Resolver {
    pub(crate) fn new(solver: Solver, substitution: Subst) -> Self {
        Self { engine: solver.engine().clone(), solver, substitution }
    }

    pub(crate) const fn engine(&self) -> &TrackedEngine { &self.engine }

    /// Lowers a type from the owner's generic context.
    pub(crate) async fn lower_type(&self, ty: &Interned<Ty>) -> Interned<MonoType> {
        lower_type(&self.solver, ty, &self.substitution).await
    }

    /// The function that a direct call of `def_id` with `substitution` calls:
    /// the `extern def` itself, or else the definition function of `def_id`.
    pub(crate) async fn direct_callee(
        &self,
        def_id: GlobalSymbolID,
        substitution: Subst,
    ) -> MonoFunctionInstance {
        // An `extern def` has no IR and cannot be polymorphic.
        if self.engine.get_symbol_kind(def_id).await == SymbolKind::ExternDef {
            return MonoFunctionInstance::ExternDef(def_id);
        }

        self.definition_instance(def_id, substitution).await.into()
    }

    /// Normalizes a callee's arguments into the key of its definition
    /// function.
    pub(crate) async fn definition_instance(
        &self,
        def_id: GlobalSymbolID,
        substitution: Subst,
    ) -> MonoBodyInstance {
        MonoBodyInstance::new_definition(def_id, substitution, &self.solver).await
    }

    /// Resolves an abstract trait-method call to a definition or nominal
    /// closure.
    pub(crate) async fn resolve_instance_call(
        &self,
        dictionary_ty: &Interned<Ty>,
        trait_def_id: GlobalSymbolID,
        trait_call_substitution: &Subst,
    ) -> InstanceCallable {
        // The dictionary may still be a polymorphic variable owned by the
        // function being monomorphized. Applying the owner's substitution turns
        // it into the concrete instance chosen at the caller, such as `EqInt`.
        let dictionary_ty = dictionary_ty.apply_subst_or_clone(&self.substitution, &self.engine);
        let dictionary_ty = self.solver.normalize(&dictionary_ty).await;
        let Ty::Application(application) = &*dictionary_ty else {
            panic!("an instance call should resolve to a concrete instance application")
        };
        match application.view() {
            ApplicationView::Instance(instance) => {
                self.resolve_definition_call(instance, trait_def_id, trait_call_substitution).await
            }
            ApplicationView::DefInstance(closure_ty) => {
                self.resolve_closure_call(closure_ty, trait_def_id).await
            }
            // Dropping a coroutine is not lowered yet: it does nothing, so what
            // the frame holds leaks.
            ApplicationView::NoOpDropInstance(_) | ApplicationView::CoroutineDropInstance(_) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);
                InstanceCallable::NoOp
            }
            ApplicationView::TupleDropInstance(instance) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);
                InstanceCallable::TupleDrop(instance.element_instances().to_vec())
            }
            ApplicationView::ClosureDropInstance(instance) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);
                let captures = self.closure_capture_ids(instance.closure()).await;
                assert_eq!(captures.len(), instance.capture_instances().len());
                InstanceCallable::ClosureDrop(
                    captures
                        .into_iter()
                        .zip(instance.capture_instances().iter().cloned())
                        .collect(),
                )
            }
            ApplicationView::GeneratedAdtDropInstance(_) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);

                // The generated body is a separate function so recursive
                // nominal types do not expand indefinitely at the call site.
                InstanceCallable::NominalDrop(
                    MonoGeneratedAdtDropInstance::new(&dictionary_ty, &self.solver).await,
                )
            }
            ApplicationView::CoroutineInstance(_) => {
                panic!("the `Coroutine` trait has no method to call")
            }
            ApplicationView::ContinuationDefInstance(continuation) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DefCall).await);
                let coroutine = continuation.coroutine_instance();
                InstanceCallable::Continuation(self.resolve_coroutine_dictionary(coroutine).await)
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Enum(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefCoroutine(_)
            | ApplicationView::TraitDefCoroutine(_)
            | ApplicationView::ClosureCoroutine(_)
            | ApplicationView::Error => {
                panic!("an instance call should resolve to a type of instance kind")
            }
        }
    }

    /// Resolves the `Drop.drop` implementation selected by a stored dictionary.
    pub(crate) async fn resolve_drop_call(&self, dictionary_ty: &Interned<Ty>) -> InstanceCallable {
        let drop_method = self.engine.get_core_item(CoreItem::DropMethod).await;
        self.resolve_instance_call(dictionary_ty, drop_method, &Subst::new_empty()).await
    }

    /// The ABI of a generated `Drop.drop`, matching `drop(self: T) -> unit`.
    pub(crate) async fn nominal_drop_signature(&self, nominal: &Interned<Ty>) -> FunctionSignature {
        let parameter = self.lower_type(nominal).await;
        let unit = MonoType::new_unit(&self.engine);
        FunctionSignature::builder()
            .parameter_types(self.engine.intern_unsized([parameter]))
            .return_type(ReturnType::Value(unit))
            .engine(&self.engine)
            .build()
    }

    /// Selects a nominal body and its ABI from a built-in Def dictionary.
    async fn resolve_closure_call(
        &self,
        closure_ty: &Interned<Ty>,
        trait_def_id: GlobalSymbolID,
    ) -> InstanceCallable {
        // Only core.Def.call can invoke a built-in nominal closure dictionary.
        assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DefCall).await);
        let Ty::Application(closure) = &**closure_ty else {
            panic!("DefInstance requires a closure")
        };
        let ApplicationView::Closure(closure) = closure.view() else {
            panic!("DefInstance requires a nominal closure")
        };

        // Leave body discovery to the backend worklist to allow cycles.
        let instance = MonoBodyInstance::from_closure(&self.solver, closure).await;
        let signature = self.callback_signature(&instance).await;
        InstanceCallable::Closure(instance, signature)
    }

    /// The captures of the closure of type `closure_ty`, in the order of its
    /// captured tuple, which is the order of its capture map.
    async fn closure_capture_ids(&self, closure_ty: &Interned<Ty>) -> Vec<CaptureID> {
        let closure = closure_ty.unwrap_as_closure_view();
        let instance = MonoBodyInstance::from_closure(&self.solver, closure).await;
        let source = self.engine.get_ir(instance.def_id()).await;
        source
            .get_captures_of_function(instance.ir_function_id())
            .map(|(capture, _)| capture)
            .collect()
    }

    /// Maps a trait method into the concrete instance and method namespaces.
    async fn resolve_definition_call(
        &self,
        instance: InstanceView<'_>,
        trait_def_id: GlobalSymbolID,
        trait_call_substitution: &Subst,
    ) -> InstanceCallable {
        // Trait and instance methods correspond by name. The InstanceMember
        // query verifies that correspondence and provides the
        // precomputed mapping from trait-owned polymorphic variables to
        // instance-owned variables.
        let trait_def_name = self.engine.get_name(trait_def_id).await;
        let instance_def_id = self
            .engine
            .get_member_by_name(instance.symbol_id(), &trait_def_name)
            .await
            .expect("a concrete instance should implement the selected trait definition");
        let instance_def = self
            .engine
            .get_instance_member(instance_def_id)
            .await
            .expect("a checked method has a correspondence");
        assert_eq!(
            instance_def.trait_member_id(),
            trait_def_id,
            "the selected instance definition should implement the called trait definition"
        );

        // Seed the callee substitution with arguments belonging to the instance
        // declaration itself. For `SomeInstance[int32]`, this maps the
        // instance's type variables to `int32`.
        let instance_args = Args::new(instance.args().iter().cloned(), &self.engine);
        let mut substitution =
            self.engine.build_subst_from_args(instance.symbol_id(), &instance_args).await;

        // Method-local variables have different IDs in the trait declaration
        // and its instance implementation. First make the trait call
        // substitution concrete, then use InstanceMember's mapping to
        // move each value into the corresponding instance-method
        // variable.
        let mut trait_call_substitution = trait_call_substitution.clone();
        trait_call_substitution.compose(&self.substitution, &self.engine);
        let trait_def_poly_vars = self.engine.get_poly_var_map(trait_def_id).await;
        let local_substitution = trait_def_poly_vars
            .iter()
            .filter_map(|(trait_poly_var_id, _)| {
                let trait_poly_var_id = GlobalPolyVarID::new(trait_def_id, trait_poly_var_id);
                let concrete_ty = trait_call_substitution.get(&trait_poly_var_id)?;
                let instance_poly_var =
                    instance_def.poly_var_substitution().get(&trait_poly_var_id)?;
                let Ty::PolyVar(instance_poly_var_id) = &**instance_poly_var else {
                    panic!(
                        "a trait definition variable should map to an instance definition variable"
                    )
                };
                Some((*instance_poly_var_id, concrete_ty.clone()))
            })
            .collect();
        substitution.compose(&local_substitution, &self.engine);

        // From this point on, the call is indistinguishable from any other
        // call of a definition: a concrete definition ID plus its substitution.
        InstanceCallable::Definition(self.definition_instance(instance_def_id, substitution).await)
    }

    /// The coroutine that a call resumes if `dictionary` makes it a call of a
    /// continuation, rather than of a function with a frame of its own.
    pub(crate) async fn resolve_continuation_dictionary(
        &self,
        dictionary: &Interned<Ty>,
    ) -> Option<MonoCoroutineInstance> {
        let dictionary = dictionary.apply_subst_or_clone(&self.substitution, &self.engine);
        let dictionary = self.solver.normalize(&dictionary).await;
        let Ty::Application(application) = &*dictionary else {
            return None;
        };
        let ApplicationView::ContinuationDefInstance(continuation) = application.view() else {
            return None;
        };

        Some(self.resolve_coroutine_dictionary(continuation.coroutine_instance()).await)
    }

    /// The coroutine behind a `Coroutine` dictionary, which is written for a
    /// coroutine type or a `&mut` to one.
    pub(crate) async fn resolve_coroutine_dictionary(
        &self,
        dictionary: &Interned<Ty>,
    ) -> MonoCoroutineInstance {
        let dictionary = dictionary.apply_subst_or_clone(&self.substitution, &self.engine);
        let dictionary = self.solver.normalize(&dictionary).await;
        let Ty::Application(application) = &*dictionary else {
            panic!("a `Coroutine` dictionary should be a concrete application")
        };
        let ApplicationView::CoroutineInstance(coroutine) = application.view() else {
            panic!("a `Coroutine` dictionary should be the built-in instance")
        };
        let coroutine =
            coroutine.as_reference_view().map_or(coroutine, |reference| reference.pointee());

        MonoCoroutineInstance::from_type(coroutine, &self.solver).await
    }

    pub(crate) async fn coroutine_signature(
        &self,
        instance: &MonoCoroutineInstance,
    ) -> Interned<CoroutineSignature> {
        self.engine.get_coroutine_signature(instance.clone()).await
    }

    pub(crate) async fn effect_row_layout(&self, row: &EffectRow) -> Interned<EffectRowLayout> {
        self.engine.build_effect_row_layout(row.clone()).await
    }

    pub(crate) async fn struct_layout(
        &self,
        instance: &MonoStructInstance,
    ) -> Interned<StructLayout> {
        self.engine.build_struct_layout(instance.clone()).await
    }

    pub(crate) async fn frame_layout(
        &self,
        instance: &MonoCoroutineInstance,
    ) -> Interned<FrameLayout> {
        self.engine.build_frame_layout(instance.clone()).await
    }

    pub(crate) async fn frame_field_types(
        &self,
        instance: &MonoCoroutineInstance,
    ) -> Interned<FrameFieldTypes> {
        self.engine.build_frame_field_types(instance.clone()).await
    }

    /// The calling signature of a function of a definition's IR, including the
    /// effects whose handlers the caller must pass.
    pub(crate) async fn callback_signature(
        &self,
        instance: &MonoBodyInstance,
    ) -> FunctionSignature {
        self.engine.get_callback_signature(instance.clone()).await
    }

    /// The calling signature of any function, which a caller spells without
    /// lowering it.
    pub(crate) async fn function_signature(
        &self,
        instance: &MonoFunctionInstance,
    ) -> FunctionSignature {
        match instance {
            MonoFunctionInstance::Body(body) => self.callback_signature(body).await,
            MonoFunctionInstance::ExternDef(def_id) => {
                self.engine.get_extern_def_signature(*def_id).await
            }
            MonoFunctionInstance::NominalDrop(drop) => {
                self.nominal_drop_signature(drop.view().nominal()).await
            }
            MonoFunctionInstance::CoroutineResume(coroutine) => {
                self.coroutine_signature(coroutine).await.resume_signature().clone()
            }
        }
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
        owner_substituion: &Subst,
    ) -> MonoEffectInstance {
        self.solver.instantiate_effect(effect_id, substitution, owner_substituion).await
    }
}
