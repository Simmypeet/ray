//! Where each value and block of a semantic IR function went in `MonoIR`.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local as IRLocal,
    cfg::BlockID as IRBlockID,
    ir_closure::{CaptureID, ClosureParameterID},
    ir_expr::IRExprID,
    ir_function::{IRContext, IRFunctionMap},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    MonoBodyInstance,
    cfg::BlockID,
    function::LocalID,
    layout::EnvironmentLayout,
    operand::Operand,
    place::Place,
    rvalue::{Cast, Rvalue},
    ty::{EnvironmentSlot, MonoType, PointerMutability},
};
use rayc_semantic_element::parameter::{ParameterID, get_parameter_map};

use super::lowering_style::Frame;
use crate::emitter::Emitter;

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
    /// environment, whose layout is `environment`, if it has anything in it.
    pub(super) async fn new_callback(
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &mut Emitter<'_>,
        environment: Option<&Interned<EnvironmentLayout>>,
    ) -> (Self, Option<Place>) {
        let mut map = Self::empty();
        map.bind_parameters(source, body, emit).await;

        map.initialize_storage(source, body, emit, None).await;
        map.initialize_blocks(source, body, emit, false);
        let environment = environment.and_then(|layout| map.bind_environment(emit, layout));

        (map, environment)
    }

    /// Maps the values of a function in coroutine style to `frame`, and
    /// everything else to locals of `coroutine_resume`.
    ///
    /// Also moves the start fields to locals at the top of the block of the
    /// source entry, since the first value stored in a suspension variant may
    /// overwrite them. Only the first resume runs them: the dispatch enters
    /// that block when the coroutine has not started, and no block of a
    /// source function jumps back to its entry.
    pub(super) async fn new_coroutine(
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &mut Emitter<'_>,
        frame: &Frame,
    ) -> Self {
        let mut map = Self::empty();
        map.initialize_blocks(source, body, emit, true);

        // Binding the upvars emits the moves into the selected block.
        emit.select_block(map.block(source.get_function(body.ir_function_id()).entry_block()));
        map.bind_frame_upvars(source, body, emit, frame).await;
        map.initialize_storage(source, body, emit, Some(frame)).await;

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
    /// target. `LoweringStyle` maps the handler-record parameters.
    async fn bind_parameters(
        &mut self,
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &Emitter<'_>,
    ) {
        let mut parameters = emit.parameter_ids();
        let context = source.get_function(body.ir_function_id()).context();

        match context {
            IRContext::Def => {
                for ((parameter_id, _), target_id) in emit
                    .engine()
                    .get_parameter_map(body.def_id())
                    .await
                    .iter()
                    .zip(parameters.by_ref())
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
        assert_eq!(
            emit.environment_parameter().is_some(),
            context.capture_map_id().is_some(),
            "a function should have an environment slot exactly when it has captures"
        );
    }

    /// Maps the parameters and captures, which always start in the frame, or
    /// for borrowed captures, in the environment that the frame points to.
    async fn bind_frame_upvars(
        &mut self,
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &mut Emitter<'_>,
        frame: &Frame,
    ) {
        let function = source.get_function(body.ir_function_id());

        let locals = function.parameter_locals(body.def_id(), emit.engine()).await;
        for local in locals {
            let place = Self::bind_frame_upvar(Savable::Local(local), emit, frame);
            match local {
                IRLocal::Parameter(parameter) => {
                    self.parameters.insert(parameter, place);
                }
                IRLocal::ClosureParameter(parameter) => {
                    self.closure_parameters.insert(parameter, place);
                }
                IRLocal::OperationHandlerParameter(parameter) => {
                    self.operation_parameters.insert(parameter, place);
                }
                IRLocal::Variable(_) | IRLocal::Capture(_) => {
                    panic!("only a parameter starts a function")
                }
            }
        }

        // A definition has no captures.
        if function.context().capture_map_id().is_none() {
            return;
        }

        // The clauses of a `run ... with` share their environment, which the
        // frame points to: `(*frame->environment).capture`.
        let shared_environment = function.context().borrows_captures().then(|| {
            let pointer = frame
                .place_of(Savable::Environment, emit.engine())
                .expect("the frame of a function that borrows its captures points to them");
            emit.project(&pointer).dereference()
        });

        for (capture, _) in source.get_captures_of_function(body.ir_function_id()) {
            let place = match &shared_environment {
                Some(environment) => emit.project(environment).environment_capture(capture),
                None => {
                    Self::bind_frame_upvar(Savable::Local(IRLocal::Capture(capture)), emit, frame)
                }
            };
            self.captures.insert(capture, place);
        }
    }

    /// The place the function reads a parameter or capture from: its field in
    /// the frame, or for a start field, a local that the selected block moves
    /// it to.
    fn bind_frame_upvar(savable: Savable, emit: &mut Emitter<'_>, frame: &Frame) -> Place {
        let field = frame
            .place_of(savable, emit.engine())
            .expect("a frame holds every parameter and capture");
        if !frame.is_start_field(savable) {
            return field;
        }

        let local = emit.new_variable_place(frame.field_type(savable).clone());
        emit.store(local.clone(), Operand::Copy(field));
        local
    }

    /// Maps the variables and expressions of the source function to their
    /// storage: a field of `frame` if it holds them, or a new local otherwise.
    async fn initialize_storage(
        &mut self,
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &mut Emitter<'_>,
        frame: Option<&Frame>,
    ) {
        let function = source.get_function(body.ir_function_id());
        let resolver = emit.resolver();
        let engine = emit.engine();
        let frame_place = |savable| frame?.place_of(savable, engine);

        for (variable_id, variable) in function.variables() {
            let savable = Savable::Local(IRLocal::Variable(variable_id));
            let place = if let Some(place) = frame_place(savable) {
                place
            } else {
                emit.new_variable_place(resolver.lower_type(variable.ty()).await)
            };
            self.variables.insert(variable_id, place);
        }
        for expression_id in function.direct_reachables().expressions() {
            let ty = resolver.lower_type(function.get_expression(expression_id).ty()).await;
            let place = frame_place(Savable::Expression(expression_id))
                .unwrap_or_else(|| emit.new_temporary_place(ty.clone()));
            self.expressions.insert(expression_id, (place, ty));
        }
    }

    /// Creates a target block for every reachable block. The source entry takes
    /// the target entry, unless the target function starts with a dispatch.
    fn initialize_blocks(
        &mut self,
        source: &IRFunctionMap,
        body: &MonoBodyInstance,
        emit: &mut Emitter<'_>,
        entry_is_dispatch: bool,
    ) {
        let function = source.get_function(body.ir_function_id());
        for source_block in function.direct_reachables().blocks() {
            let target_block = if source_block == function.entry_block() && !entry_is_dispatch {
                emit.entry_block()
            } else {
                emit.create_block()
            };
            self.blocks.insert(source_block, target_block);
        }
    }

    /// Maps the captures to the fields of the environment, whose layout is
    /// `layout`, that the environment parameter passes, and returns the place
    /// of the environment if it has anything in it.
    fn bind_environment(
        &mut self,
        emit: &mut Emitter<'_>,
        layout: &Interned<EnvironmentLayout>,
    ) -> Option<Place> {
        const HAS_SLOT: &str = "a function with captures should have an environment slot";

        if layout.is_empty() {
            return None;
        }

        let environment_parameter = emit.environment_parameter().expect(HAS_SLOT);
        let environment = match emit.environment_slot().expect(HAS_SLOT) {
            // no need to dereference the environment parameter, it is already
            // the correct type
            EnvironmentSlot::ByValue(_) => emit.local_place(environment_parameter),
            EnvironmentSlot::Opaque => {
                cast_environment_pointer(layout, emit, environment_parameter)
            }
        };

        for capture_id in layout.capture_ids() {
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
    layout: &EnvironmentLayout,
    emit: &mut Emitter<'_>,
    environment_parameter: LocalID,
) -> Place {
    let engine = emit.engine();
    let pointer_type = MonoType::new_pointer(
        engine.intern(layout.environment_type()),
        PointerMutability::Mut,
        engine,
    );

    let pointer = emit.new_temporary_place(pointer_type.clone());
    let parameter = Operand::Copy(emit.local_place(environment_parameter));
    emit.assign(pointer.clone(), Rvalue::Cast(Cast::new(parameter, pointer_type)));

    emit.project(&pointer).dereference()
}
