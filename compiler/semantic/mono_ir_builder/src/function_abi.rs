use rayc_ir::{
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureID,
};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::MonoFunctionKind,
    ty::{Environment as EnvironmentTy, FunctionSignature},
};

use crate::context::Context;

/// How the hidden capture parameter is passed.
#[derive(Debug, Clone)]
enum EnvironmentPassing {
    None,
    ErasedPointer(EnvironmentTy),
    ByValue(EnvironmentTy),
}

/// The concrete calling convention and environment layout of one function.
#[derive(Debug, Clone)]
pub(crate) struct FunctionABI {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
    effects: Vec<MonoEffectInstance>,
    environment_type: EnvironmentPassing,
    capture_ids: Vec<CaptureID>,
}

impl FunctionABI {
    const fn new(
        kind: MonoFunctionKind,
        signature: FunctionSignature,
        effects: Vec<MonoEffectInstance>,
        environment_type: EnvironmentPassing,
        capture_ids: Vec<CaptureID>,
    ) -> Self {
        Self { kind, signature, effects, environment_type, capture_ids }
    }

    pub(crate) const fn by_value(&self) -> bool {
        matches!(self.environment_type, EnvironmentPassing::ByValue(_))
    }

    pub(crate) const fn kind(&self) -> MonoFunctionKind { self.kind }

    pub(crate) const fn signature(&self) -> &FunctionSignature { &self.signature }

    pub(crate) fn effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.effects.iter()
    }

    pub(crate) const fn environment_type(&self) -> &EnvironmentTy {
        match &self.environment_type {
            EnvironmentPassing::None => panic!("function has no environment"),
            EnvironmentPassing::ErasedPointer(environment)
            | EnvironmentPassing::ByValue(environment) => environment,
        }
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
    pub(super) async fn plan_function(
        &self,
        source_id: rayc_ir::ir_function::FunctionID,
        source: &IRFunction,
    ) -> FunctionABI {
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
                // Reserve the hidden environment slot; nominal bodies replace
                // this erased parameter with their shared by-value ABI below.
                parameter_types.push(self.create_opaque_pointer());

                for (_, parameter) in context.parameters() {
                    parameter_types.push(self.lower_type(parameter.ty()).await);
                }

                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_capture_storage_type(capture).await);
                }

                (MonoFunctionKind::Lambda, self.lower_type(context.return_ty()).await)
            }

            IRContext::Thunk(context) => {
                // The first parameter carries the erased capture environment.
                parameter_types.push(self.create_opaque_pointer());

                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_capture_storage_type(capture).await);
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
                    environment_fields.push(self.lower_capture_storage_type(capture).await);
                }

                for effect in &effects {
                    environment_fields.push(self.create_handler_pointer(effect.clone()));
                }

                (MonoFunctionKind::OperationHandler, self.lower_type(context.return_ty()).await)
            }
        };

        // Nominal bodies and dictionary calls share the same environment/handler ABI.
        let (signature, environment_type) = if self.is_nominal(source_id) {
            let environment =
                self.create_aggregate_type_for_capture_environment(environment_fields);
            parameter_types.remove(0);
            let signature = self.create_nominal_signature(
                environment.clone(),
                parameter_types,
                return_type,
                &effects,
            );
            (signature, EnvironmentPassing::ByValue(environment))
        } else {
            // Operation callbacks capture handlers instead of receiving them.
            if kind != MonoFunctionKind::OperationHandler {
                parameter_types.extend(
                    effects.iter().map(|effect| self.create_handler_pointer(effect.clone())),
                );
            }
            let environment = if kind == MonoFunctionKind::Def {
                EnvironmentPassing::None
            } else {
                EnvironmentPassing::ErasedPointer(
                    self.create_aggregate_type_for_capture_environment(environment_fields),
                )
            };
            (self.create_function_signature(parameter_types, return_type), environment)
        };
        FunctionABI::new(kind, signature, effects, environment_type, capture_ids)
    }
}
