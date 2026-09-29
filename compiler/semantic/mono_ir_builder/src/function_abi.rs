use rayc_arena::ID;
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
#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvironmentPassing {
    ErasedPointer(EnvironmentTy),
    ByValue(EnvironmentTy),
}

/// The concrete layout and passing convention of one capture environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvironmentABI {
    passing: EnvironmentPassing,
    capture_ids: Vec<CaptureID>,
    captured_effects: Vec<MonoEffectInstance>,
}

pub(crate) type EnvironmentABIID = ID<EnvironmentABI>;

impl EnvironmentABI {
    const fn new(
        passing: EnvironmentPassing,
        capture_ids: Vec<CaptureID>,
        captured_effects: Vec<MonoEffectInstance>,
    ) -> Self {
        Self { passing, capture_ids, captured_effects }
    }

    pub(crate) const fn by_value(&self) -> bool {
        matches!(self.passing, EnvironmentPassing::ByValue(_))
    }

    pub(crate) const fn environment_type(&self) -> &EnvironmentTy {
        match &self.passing {
            EnvironmentPassing::ErasedPointer(environment)
            | EnvironmentPassing::ByValue(environment) => environment,
        }
    }

    pub(crate) fn capture_ids(&self) -> impl ExactSizeIterator<Item = CaptureID> + '_ {
        self.capture_ids.iter().copied()
    }

    pub(crate) const fn capture_count(&self) -> usize { self.capture_ids.len() }

    pub(crate) fn captured_effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.captured_effects.iter()
    }
}

/// The concrete calling convention of one function and its environment ABI
/// reference.
#[derive(Debug, Clone)]
pub(crate) struct FunctionABI {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
    effects: Vec<MonoEffectInstance>,
    environment_id: Option<EnvironmentABIID>,
}

impl FunctionABI {
    const fn new(
        kind: MonoFunctionKind,
        signature: FunctionSignature,
        effects: Vec<MonoEffectInstance>,
        environment_id: Option<EnvironmentABIID>,
    ) -> Self {
        Self { kind, signature, effects, environment_id }
    }

    pub(crate) const fn kind(&self) -> MonoFunctionKind { self.kind }

    pub(crate) const fn signature(&self) -> &FunctionSignature { &self.signature }

    pub(crate) fn effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.effects.iter()
    }

    pub(crate) const fn environment_id(&self) -> EnvironmentABIID {
        self.environment_id.expect("function should have a capture environment ABI")
    }

    pub(crate) fn captures_effect_handlers(&self) -> bool {
        self.kind == MonoFunctionKind::OperationHandler
    }
}

impl Context {
    pub(super) async fn plan_function(
        &mut self,
        source_id: rayc_ir::ir_function::FunctionID,
        source: &IRFunction,
    ) -> FunctionABI {
        let effects = self.lower_source_effects(source_id).await;

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
                // Every closure body uses the nominal environment ABI below.

                for (_, parameter) in context.parameters() {
                    parameter_types.push(self.lower_type(parameter.ty()).await);
                }

                for (capture_id, capture) in self.source_captures(source_id) {
                    capture_ids.push(capture_id);
                    environment_fields.push(self.lower_capture_storage_type(capture).await);
                }

                (MonoFunctionKind::Lambda, self.lower_type(context.return_ty()).await)
            }

            IRContext::Thunk(context) => {
                // The first parameter carries the erased capture environment.
                parameter_types.push(self.create_opaque_pointer());

                for (capture_id, capture) in self.source_captures(source_id) {
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
                for (capture_id, capture) in self.source_captures(source_id) {
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
        let (signature, environment_passing) = if self.is_nominal(source_id) {
            let environment =
                self.create_aggregate_type_for_capture_environment(environment_fields);
            let signature = self.create_nominal_signature(
                environment.clone(),
                parameter_types,
                return_type,
                &effects,
            );
            (signature, Some(EnvironmentPassing::ByValue(environment)))
        } else {
            // Operation callbacks capture handlers instead of receiving them.
            if kind != MonoFunctionKind::OperationHandler {
                parameter_types.extend(
                    effects.iter().map(|effect| self.create_handler_pointer(effect.clone())),
                );
            }
            let environment = if kind == MonoFunctionKind::Def {
                None
            } else {
                Some(EnvironmentPassing::ErasedPointer(
                    self.create_aggregate_type_for_capture_environment(environment_fields),
                ))
            };
            (self.create_function_signature(parameter_types, return_type), environment)
        };

        // Capture environments are planned independently from function signatures so
        // functions sharing a capture map also share one environment ABI.
        let environment_id = environment_passing.map(|passing| {
            let captured_effects = if kind == MonoFunctionKind::OperationHandler {
                effects.clone()
            } else {
                Vec::new()
            };
            let environment = EnvironmentABI::new(passing, capture_ids, captured_effects);
            self.plan_capture_environment(self.source_capture_map_id(source_id), environment)
        });

        FunctionABI::new(kind, signature, effects, environment_id)
    }
}
