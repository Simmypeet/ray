use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureID,
};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::MonoFunctionKind,
    ty::{AggregateKind, AggregateType, FunctionSignature, MonoType},
};

use crate::builder::Builder;

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

impl Builder {
    pub(super) async fn plan_function(&mut self, source: &IRFunction) -> FunctionABI {
        let effects = self.types.lower_effects(source.effect(), self.instance.substitution()).await;

        let mut parameter_types = Vec::new();
        let mut capture_ids = Vec::new();
        let mut environment_fields = Vec::new();

        let (kind, return_type) = match source.context() {
            IRContext::Def => {
                for (_, parameter) in self.root_parameters.iter() {
                    parameter_types.push(
                        self.types.lower_type(parameter.ty(), self.instance.substitution()).await,
                    );
                }
                (MonoFunctionKind::Def, self.root_return_type.clone())
            }
            IRContext::Lambda(context) => {
                // The first parameter carries the erased capture environment.
                parameter_types.push(self.types.opaque_pointer());

                for (_, parameter) in context.parameters() {
                    parameter_types.push(
                        self.types.lower_type(parameter.ty(), self.instance.substitution()).await,
                    );
                }

                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(
                        self.types
                            .lower_type(
                                &capture.pointer_ty(&self.engine),
                                self.instance.substitution(),
                            )
                            .await,
                    );
                }
                (MonoFunctionKind::Lambda, context.return_ty().clone())
            }
            IRContext::Thunk(context) => {
                parameter_types.push(self.types.opaque_pointer());
                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(
                        self.types
                            .lower_type(
                                &capture.pointer_ty(&self.engine),
                                self.instance.substitution(),
                            )
                            .await,
                    );
                }
                (MonoFunctionKind::Thunk, context.return_ty().clone())
            }
            IRContext::OperationHandler(context) => {
                parameter_types.push(self.types.opaque_pointer());
                for (_, parameter) in context.parameters() {
                    parameter_types.push(
                        self.types.lower_type(parameter.ty(), self.instance.substitution()).await,
                    );
                }
                for (capture_id, capture) in context.captures() {
                    capture_ids.push(capture_id);
                    environment_fields.push(
                        self.types
                            .lower_type(
                                &capture.pointer_ty(&self.engine),
                                self.instance.substitution(),
                            )
                            .await,
                    );
                }
                for effect in &effects {
                    environment_fields.push(self.types.handler_pointer(effect.clone()));
                }
                (MonoFunctionKind::OperationHandler, context.return_ty().clone())
            }
        };

        // Ordinary functions receive their handlers as hidden parameters.
        // Operation-handler callbacks capture them in their environments so
        // their signatures continue to match the effect operation signatures.
        if kind != MonoFunctionKind::OperationHandler {
            for effect in &effects {
                parameter_types.push(self.types.handler_pointer(effect.clone()));
            }
        }

        let return_type = self.types.lower_type(&return_type, self.instance.substitution()).await;
        let signature = self.types.signature(parameter_types, return_type);
        let environment_type = (kind != MonoFunctionKind::Def).then(|| {
            self.types.intern(MonoType::Aggregate(AggregateType::new(
                AggregateKind::CaptureEnvironment,
                environment_fields.iter().map(|ty| (**ty).clone()).collect(),
            )))
        });

        FunctionABI::new(kind, signature, effects, environment_type, capture_ids)
    }
}
