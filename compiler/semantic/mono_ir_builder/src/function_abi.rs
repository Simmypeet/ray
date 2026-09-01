use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureID,
};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::MonoFunctionKind,
    ty::{FunctionSignature, MonoType},
};

use crate::context::Context;

/// The concrete calling convention and environment layout of one function.
#[derive(Debug, Clone)]
pub(crate) struct FunctionABI {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
    effects: Vec<MonoEffectInstance>,
    environment_type: Option<Interned<MonoType>>,
    capture_ids: Vec<CaptureID>,
}

impl FunctionABI {
    const fn new(
        kind: MonoFunctionKind,
        signature: FunctionSignature,
        effects: Vec<MonoEffectInstance>,
        environment_type: Option<Interned<MonoType>>,
        capture_ids: Vec<CaptureID>,
    ) -> Self {
        Self { kind, signature, effects, environment_type, capture_ids }
    }

    pub(crate) const fn kind(&self) -> MonoFunctionKind { self.kind }

    pub(crate) const fn signature(&self) -> &FunctionSignature { &self.signature }

    pub(crate) fn effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.effects.iter()
    }

    pub(crate) fn environment_type(&self) -> Interned<MonoType> {
        self.environment_type
            .as_ref()
            .expect("nested function should have an environment type")
            .clone()
    }

    pub(crate) fn capture_ids(&self) -> impl ExactSizeIterator<Item = CaptureID> + '_ {
        self.capture_ids.iter().copied()
    }

    pub(crate) const fn capture_count(&self) -> usize { self.capture_ids.len() }

    pub(crate) fn captures_effect_handlers(&self) -> bool {
        self.kind == MonoFunctionKind::OperationHandler
    }
}

impl Context {
    pub(super) async fn plan_function(&self, source: &IRFunction) -> FunctionABI {
        let effects = self.lower_effects(source.effect()).await;

        let mut parameter_types = Vec::new();
        let mut capture_ids = Vec::new();
        let mut environment_fields = Vec::new();

        let (kind, return_type) = match source.context() {
            IRContext::Def => {
                let parameters = self.get_root_parameter_map().await;
                for (_, parameter) in parameters.iter() {
                    parameter_types.push(self.lower_type(parameter.ty()).await);
                }

                (MonoFunctionKind::Def, self.lower_type(&self.get_root_return_type().await).await)
            }

            IRContext::Lambda(context) => {
                // The first parameter carries the erased capture environment.
                parameter_types.push(self.create_opaque_pointer());

                for (_, parameter) in context.parameters() {
                    parameter_types.push(self.lower_type(parameter.ty()).await);
                }

                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_pointer_type_for_capture(capture).await);
                }

                (MonoFunctionKind::Lambda, self.lower_type(context.return_ty()).await)
            }

            IRContext::Thunk(context) => {
                // The first parameter carries the erased capture environment.
                parameter_types.push(self.create_opaque_pointer());

                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_pointer_type_for_capture(capture).await);
                }

                (MonoFunctionKind::Thunk, self.lower_type(context.return_ty()).await)
            }
            IRContext::OperationHandler(context) => {
                parameter_types.push(self.create_opaque_pointer());

                for (_, parameter) in context.parameters() {
                    parameter_types.push(self.lower_type(parameter.ty()).await);
                }
                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_pointer_type_for_capture(capture).await);
                }

                for effect in &effects {
                    environment_fields.push(self.create_handler_pointer(effect.clone()));
                }

                (MonoFunctionKind::OperationHandler, self.lower_type(context.return_ty()).await)
            }
        };

        // Ordinary functions receive their handlers as hidden parameters.
        // Operation-handler callbacks capture them in their environments so
        // their signatures continue to match the effect operation signatures.
        if kind != MonoFunctionKind::OperationHandler {
            for effect in &effects {
                parameter_types.push(self.create_handler_pointer(effect.clone()));
            }
        }

        let signature = self.create_function_signature(parameter_types, return_type);
        let environment_type = (kind != MonoFunctionKind::Def)
            .then(|| self.create_aggregate_type_for_capture_environment(environment_fields));

        FunctionABI::new(kind, signature, effects, environment_type, capture_ids)
    }
}
