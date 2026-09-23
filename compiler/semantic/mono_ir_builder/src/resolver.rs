//! Resolves dictionaries and callee signatures in one concrete owner.
//!
//! Both source-definition fragments and generated nominal Drop fragments call
//! through dictionaries. The owner substitution is empty for a generated Drop
//! fragment because its dictionary key is already concrete.
//!
//! The resolver owns the fragment's single `Solver`; every normalization while
//! lowering the fragment goes through it rather than a fresh solver.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoEffectInstance, MonoNominalDropInstance,
    ty::{FunctionSignature, MonoType, ReturnType, lower_effects, lower_type, nominal_signature},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_member_by_name,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
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
    Definition(MonoDefInstance),
    Closure(MonoClosureInstance, FunctionSignature, Vec<MonoEffectInstance>),
    TupleDrop(Vec<Interned<Ty>>),
    NominalDrop(MonoNominalDropInstance, FunctionSignature),
    NoOp,
}

/// Instance-resolution state shared by every function in one fragment.
pub(crate) struct Resolver {
    engine: TrackedEngine,
    solver: Solver,
    substitution: Subst,
}

impl Resolver {
    pub(crate) fn new(solver: Solver, substitution: Subst) -> Self {
        Self { engine: solver.engine().clone(), solver, substitution }
    }

    /// Lowers a type from the owner's generic context.
    pub(crate) async fn lower_type(&self, ty: &Interned<Ty>) -> Interned<MonoType> {
        lower_type(&self.solver, ty, &self.substitution).await
    }

    /// Lowers an effect row from the owner's generic context.
    pub(crate) async fn lower_effects(&self, effect: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        lower_effects(&self.solver, effect, &self.substitution).await
    }

    /// Normalizes a callee's arguments into a definition fragment key.
    pub(crate) async fn definition_instance(
        &self,
        def_id: GlobalSymbolID,
        substitution: Subst,
    ) -> MonoDefInstance {
        MonoDefInstance::new(def_id, substitution, &self.solver).await
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
            ApplicationView::NoOpDropInstance(_) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);
                InstanceCallable::NoOp
            }
            ApplicationView::TupleDropInstance(instance) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);
                InstanceCallable::TupleDrop(instance.element_instances().to_vec())
            }
            ApplicationView::NominalDropInstance(instance) => {
                assert_eq!(trait_def_id, self.engine.get_core_item(CoreItem::DropMethod).await);

                // The generated body is a separate fragment so recursive
                // nominal types do not expand indefinitely at the call site.
                let signature = self.nominal_drop_signature(instance.nominal()).await;
                InstanceCallable::NominalDrop(
                    MonoNominalDropInstance::new(dictionary_ty.clone()),
                    signature,
                )
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Struct(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
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
        let unit = self.unit_type().await;
        MonoType::new_function_signature([parameter], unit, &self.engine)
    }

    /// The lowered unit type, which every `Drop.drop` call returns.
    pub(crate) async fn unit_type(&self) -> Interned<MonoType> {
        lower_type(&self.solver, &Ty::new_unit(&self.engine), &Subst::new_empty()).await
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

        // Leave owner-body discovery to the backend worklist to allow fragment cycles.
        let instance = MonoClosureInstance::from_closure(&self.solver, closure).await;
        let (signature, effects) = nominal_signature(&self.solver, closure).await;
        InstanceCallable::Closure(instance, signature, effects)
    }

    /// Maps a trait method into the concrete instance and method namespaces.
    async fn resolve_definition_call(
        &self,
        instance: InstanceView<'_>,
        trait_def_id: GlobalSymbolID,
        trait_call_substitution: &Subst,
    ) -> InstanceCallable {
        // Trait and instance methods correspond by name. The InstanceMember query
        // verifies that correspondence and provides the precomputed mapping
        // from trait-owned polymorphic variables to instance-owned variables.
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
        // declaration itself. For `SomeInstance[int32]`, this maps the instance's
        // type variables to `int32`.
        let instance_args = Args::new(instance.args().iter().cloned(), &self.engine);
        let mut substitution =
            self.engine.build_subst_from_args(instance.symbol_id(), &instance_args).await;

        // Method-local variables have different IDs in the trait declaration and
        // its instance implementation. First make the trait call substitution
        // concrete, then use InstanceMember's mapping to move each value
        // into the corresponding instance-method variable.
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
        // global MonoIR call: a concrete definition ID plus its substitution.
        InstanceCallable::Definition(self.definition_instance(instance_def_id, substitution).await)
    }

    pub(crate) async fn global_signature(
        &self,
        function_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> (FunctionSignature, Vec<MonoEffectInstance>, bool) {
        let parameters = self.engine.get_parameter_map(function_id).await;
        let mut parameter_types = Vec::new();
        for (_, parameter) in parameters.iter() {
            parameter_types.push(lower_type(&self.solver, parameter.ty(), substitution).await);
        }
        let symbol_kind = self.engine.get_symbol_kind(function_id).await;
        let effects = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::InstanceDef) {
            let effect = self.engine.get_effect_row(function_id).await;
            lower_effects(&self.solver, &effect, substitution).await
        } else {
            Vec::new()
        };
        for effect in &effects {
            parameter_types.push(MonoType::new_handler_pointer(effect.clone(), &self.engine));
        }

        let return_type = self.engine.get_return_type(function_id).await;
        let return_type = lower_type(&self.solver, &return_type, substitution).await;
        let is_void = symbol_kind == SymbolKind::ExternDef && return_type.is_unit();

        let return_type = if is_void { ReturnType::Void } else { ReturnType::Value(return_type) };
        let is_variadic = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::ExternDef) {
            self.engine.is_variadic_def(function_id).await
        } else {
            false
        };
        let parameter_types = self.engine.intern_unsized(parameter_types);
        let signature = if is_variadic {
            FunctionSignature::new_variadic(parameter_types, return_type)
        } else {
            FunctionSignature::new(parameter_types, return_type)
        };
        (signature, effects, is_void)
    }
}
