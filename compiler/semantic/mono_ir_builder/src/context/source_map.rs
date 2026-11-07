//! Where each value and block of a semantic IR function went in `MonoIR`.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local as IRLocal,
    cfg::BlockID as IRBlockID,
    ir_closure::{CaptureID, ClosureParameterID},
    ir_expr::IRExprID,
    ir_function::{IRContext, IRFunctionID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    cfg::BlockID,
    function::LocalID,
    operand::Operand,
    place::Place,
    rvalue::{Cast, Rvalue},
    ty::{MonoType, PointerMutability},
};
use rayc_semantic_element::parameter::ParameterID;

use super::lowering_style::CoroutineForm;
use crate::{
    emitter::Emitter,
    fragment::{EnvironmentAbi, Fragment},
};

/// Maps semantic IR identities to the places and blocks of one target function.
pub(crate) struct SourceMap {
    blocks: FxHashMap<IRBlockID, BlockID>,
    /// The place of each expression with its type. The place alone cannot tell
    /// the type: in a coroutine, it may project a field of the frame.
    expressions: FxHashMap<IRExprID, (Place, Interned<MonoType>)>,
    variables: FxHashMap<IRVariableID, Place>,
    parameters: FxHashMap<ParameterID, Place>,
    closure_parameters: FxHashMap<ClosureParameterID, Place>,
    operation_parameters: FxHashMap<OperationHandlerParameterID, Place>,
    captures: FxHashMap<CaptureID, Place>,
}

impl SourceMap {
    /// Maps the parameters, values and blocks of a function in callback style,
    /// creating the locals and blocks they need. Also returns the place of its
    /// environment, if it has one with anything in it.
    pub(super) async fn new_callback(
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
    ) -> (Self, Option<Place>) {
        let mut map = Self::empty();
        let environment_parameter = map.bind_parameters(fragment, source_id, emit).await;

        map.initialize_storage(fragment, source_id, emit, None).await;
        map.initialize_blocks(fragment, source_id, emit, false);
        let environment = environment_parameter
            .and_then(|parameter| map.bind_environment(fragment, source_id, emit, parameter));

        (map, environment)
    }

    /// Maps the values of a function in coroutine style to the frame of `form`,
    /// and everything else to locals of `coroutine_resume`.
    ///
    /// Also moves the start fields to locals at the top of the block of the
    /// source entry, since the first value stored in a suspension variant may
    /// overwrite them. Only the first resume runs them: the dispatch enters
    /// that block when the coroutine has not started, and no block of a
    /// source function jumps back to its entry.
    pub(super) async fn new_coroutine(
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        form: &CoroutineForm,
    ) -> Self {
        let mut map = Self::empty();
        map.initialize_blocks(fragment, source_id, emit, true);

        // Binding the upvars emits the moves into the selected block.
        emit.select_block(map.block(fragment.source_function(source_id).entry_block()));
        map.bind_frame_upvars(fragment, source_id, emit, form).await;
        map.initialize_storage(fragment, source_id, emit, Some(form)).await;

        // Leave the entry block selected for lowering.
        emit.select_block(emit.entry_block());

        map
    }

    fn empty() -> Self {
        Self {
            blocks: FxHashMap::default(),
            expressions: FxHashMap::default(),
            variables: FxHashMap::default(),
            parameters: FxHashMap::default(),
            closure_parameters: FxHashMap::default(),
            operation_parameters: FxHashMap::default(),
            captures: FxHashMap::default(),
        }
    }

    /// Maps the source parameters of the source function to those of the
    /// target, and returns the parameter that holds the environment, if any.
    /// `LoweringStyle` maps the handler-record parameters.
    async fn bind_parameters(
        &mut self,
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &Emitter<'_>,
    ) -> Option<LocalID> {
        let mut parameters = emit.parameter_ids();
        let context = fragment.source_function(source_id).context();

        match context {
            IRContext::Def => {
                for ((parameter_id, _), target_id) in
                    fragment.get_root_parameter_map().await.iter().zip(parameters.by_ref())
                {
                    self.parameters.insert(parameter_id, emit.local_place(target_id));
                }
            }
            IRContext::Closure(closure) => {
                for ((parameter_id, _), target_id) in closure.parameters().zip(parameters.by_ref())
                {
                    self.closure_parameters.insert(parameter_id, emit.local_place(target_id));
                }
            }
            IRContext::Thunk(_) => {}
            IRContext::OperationHandler(handler) => {
                for ((parameter_id, _), target_id) in handler.parameters().zip(parameters.by_ref())
                {
                    self.operation_parameters.insert(parameter_id, emit.local_place(target_id));
                }
            }
        }
        assert_eq!(parameters.len(), 0, "every source parameter should be bound");

        // Every function but a definition has an environment.
        let environment_parameter = emit.environment_parameter();
        assert_eq!(
            environment_parameter.is_some(),
            context.capture_map_id().is_some(),
            "a function should have an environment slot exactly when it has captures"
        );
        environment_parameter
    }

    /// Maps the parameters and captures, which always start in the frame.
    async fn bind_frame_upvars(
        &mut self,
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        form: &CoroutineForm,
    ) {
        let source = fragment.source_function(source_id);

        let locals = source.parameter_locals(fragment.def_id(), emit.engine()).await;
        for local in locals {
            let place = Self::bind_frame_upvar(Savable::Local(local), emit, form);
            match local {
                IRLocal::Parameter(parameter) => {
                    self.parameters.insert(parameter, place);
                }
                IRLocal::ClosureParameter(parameter) => {
                    self.closure_parameters.insert(parameter, place);
                }
                IRLocal::Variable(_)
                | IRLocal::OperationHandlerParameter(_)
                | IRLocal::Capture(_) => {
                    panic!("a coroutine's function is a definition or a closure")
                }
            }
        }

        if source.context().capture_map_id().is_some() {
            for (capture, _) in fragment.source_captures(source_id) {
                let savable = Savable::Local(IRLocal::Capture(capture));
                let place = Self::bind_frame_upvar(savable, emit, form);
                self.captures.insert(capture, place);
            }
        }
    }

    /// The place the function reads a parameter or capture from: its field in
    /// the frame, or for a start field, a local that the selected block moves
    /// it to.
    fn bind_frame_upvar(savable: Savable, emit: &mut Emitter<'_>, form: &CoroutineForm) -> Place {
        let field = form
            .frame_place_of(savable, emit.engine())
            .expect("a frame holds every parameter and capture");
        if !form.is_start_field(savable) {
            return field;
        }

        let local = emit.new_variable_place(form.field_type(savable).clone());
        emit.store(local.clone(), Operand::Copy(field));
        local
    }

    /// Maps the variables and expressions of the source function to their
    /// storage: a field of the frame of `form` if it holds them, or a new local
    /// otherwise.
    async fn initialize_storage(
        &mut self,
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        form: Option<&CoroutineForm>,
    ) {
        let source = fragment.source_function(source_id);
        let resolver = fragment.resolver();
        let engine = emit.engine();
        let frame_place = |savable| form?.frame_place_of(savable, engine);

        for (variable_id, variable) in source.variables() {
            let savable = Savable::Local(IRLocal::Variable(variable_id));
            let place = if let Some(place) = frame_place(savable) {
                place
            } else {
                emit.new_variable_place(resolver.lower_type(variable.ty()).await)
            };
            self.variables.insert(variable_id, place);
        }
        for expression_id in source.direct_reachables().expressions() {
            let ty = resolver.lower_type(source.get_expression(expression_id).ty()).await;
            let place = frame_place(Savable::Expression(expression_id))
                .unwrap_or_else(|| emit.new_temporary_place(ty.clone()));
            self.expressions.insert(expression_id, (place, ty));
        }
    }

    /// Creates a target block for every reachable block. The source entry takes
    /// the target entry, unless the target function starts with a dispatch.
    fn initialize_blocks(
        &mut self,
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        entry_is_dispatch: bool,
    ) {
        let source = fragment.source_function(source_id);
        for source_block in source.direct_reachables().blocks() {
            let target_block = if source_block == source.entry_block() && !entry_is_dispatch {
                emit.entry_block()
            } else {
                emit.create_block()
            };
            self.blocks.insert(source_block, target_block);
        }
    }

    /// Maps the captures to the fields of the environment that
    /// `environment_parameter` passes, and returns the place of the environment
    /// if it has anything in it.
    fn bind_environment(
        &mut self,
        fragment: &Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        environment_parameter: LocalID,
    ) -> Option<Place> {
        let environment_abi = fragment.function_environment_abi(source_id);
        if environment_abi.layout().is_empty() {
            return None;
        }

        let environment = if environment_abi.by_value() {
            // no need to dereference the environment parameter, it is already
            // the correct type
            emit.local_place(environment_parameter)
        } else {
            cast_environment_pointer(environment_abi, emit, environment_parameter)
        };

        for capture_id in environment_abi.capture_ids() {
            let place = emit.project(&environment).environment_capture(capture_id);
            self.captures.insert(capture_id, place);
        }

        Some(environment)
    }

    pub(crate) fn block(&self, block: IRBlockID) -> BlockID {
        *self.blocks.get(&block).expect("semantic IR block should be mapped")
    }

    fn expression(&self, expression: IRExprID) -> &(Place, Interned<MonoType>) {
        self.expressions.get(&expression).expect("semantic IR expression should be mapped")
    }

    pub(crate) fn expression_place(&self, expression: IRExprID) -> Place {
        self.expression(expression).0.clone()
    }

    pub(crate) fn expression_type(&self, expression: IRExprID) -> &Interned<MonoType> {
        &self.expression(expression).1
    }

    pub(crate) fn expression_operand(&self, expression: IRExprID) -> Operand {
        Operand::Copy(self.expression_place(expression))
    }

    pub(crate) fn variable_place(&self, variable: IRVariableID) -> Place {
        self.variables.get(&variable).expect("semantic IR variable should be mapped").clone()
    }

    pub(crate) fn parameter_place(&self, parameter: ParameterID) -> Place {
        self.parameters.get(&parameter).expect("semantic parameter should be mapped").clone()
    }

    pub(crate) fn closure_parameter_place(&self, parameter: ClosureParameterID) -> Place {
        self.closure_parameters
            .get(&parameter)
            .expect("semantic closure parameter should be mapped")
            .clone()
    }

    pub(crate) fn operation_parameter_place(
        &self,
        parameter: OperationHandlerParameterID,
    ) -> Place {
        self.operation_parameters
            .get(&parameter)
            .expect("semantic operation parameter should be mapped")
            .clone()
    }

    pub(crate) fn capture_place(&self, capture: CaptureID) -> Place {
        self.captures.get(&capture).expect("semantic capture should be mapped").clone()
    }
}

/// Casts the `void* env` parameter to an `Environment* env` local, and returns
/// the place `*env` that the environment's fields are read from.
fn cast_environment_pointer(
    environment_abi: &EnvironmentAbi,
    emit: &mut Emitter<'_>,
    environment_parameter: LocalID,
) -> Place {
    let engine = emit.engine();
    let pointer_type = MonoType::new_pointer(
        engine.intern(environment_abi.environment_type()),
        PointerMutability::Mut,
        engine,
    );

    let pointer = emit.new_temporary_place(pointer_type.clone());
    let parameter = Operand::Copy(emit.local_place(environment_parameter));
    emit.assign(pointer.clone(), Rvalue::Cast(Cast::new(parameter, pointer_type)));

    emit.project(&pointer).dereference()
}
