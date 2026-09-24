use std::{cmp::Reverse, collections::BTreeMap, convert::Infallible};

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::{Address, Local, Projection},
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, Direction, JoinLattice},
    ir_expr::{
        IRExprKind,
        load::{Load, LoadKind},
    },
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureMap,
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::{parameter::get_parameter_map, struct_body::get_struct_body};
use rayc_solver::Solver;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{
    subst::Substitutable,
    ty::{Ty, application::View as ApplicationView},
    where_clause::MarkerPredicate,
};

use crate::{PlaceState, PossibleStates, StackState, stack_state::tracked_local};

/// Dataflow context for stack initialization and move state.
///
/// It borrows the IR function and its optional shared capture layout so stack
/// roots, scopes, and expressions have one source of truth. Types are
/// normalized and aggregate structure is queried lazily when an address
/// projects into that aggregate, so recursive types are never expanded eagerly.
#[derive(Debug)]
pub struct StackStateProblem<'a> {
    solver: Solver,
    function: &'a IRFunction,
    captures: Option<&'a CaptureMap>,
}

impl<'a> StackStateProblem<'a> {
    #[must_use]
    pub const fn new(
        solver: Solver,
        function: &'a IRFunction,
        captures: Option<&'a CaptureMap>,
    ) -> Self {
        Self { solver, function, captures }
    }

    pub(crate) async fn binding_type(&self, root: Local) -> Interned<Ty> {
        match root {
            Local::Variable(variable_id) => self.function.get_variable(variable_id).ty().clone(),
            Local::Parameter(parameter_id) => self
                .solver
                .engine()
                .get_parameter_map(self.solver.site())
                .await
                .iter()
                .find_map(|(id, parameter)| (id == parameter_id).then(|| parameter.ty().clone()))
                .expect("parameter address root must exist in the function signature"),
            Local::LambdaParameter(parameter_id) => self
                .function
                .context()
                .assert_as_lambda_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            Local::OperationHandlerParameter(parameter_id) => self
                .function
                .context()
                .assert_as_operation_handler_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            Local::Capture(capture_id) => self
                .captures
                .expect("capture address roots require a nested function capture layout")
                .get_capture(capture_id)
                .storage_ty(self.solver.engine()),
        }
    }

    /// Returns the declaration span of a stack root's binding.
    pub(crate) async fn binding_span(&self, root: Local) -> RelativeSpan {
        match root {
            Local::Variable(variable_id) => self.function.get_variable(variable_id).span(),
            Local::Parameter(parameter_id) => self
                .solver
                .engine()
                .get_parameter_map(self.solver.site())
                .await
                .iter()
                .find_map(|(id, parameter)| (id == parameter_id).then(|| parameter.span()))
                .expect("parameter address root must exist in the function signature")
                .expect("parameters of a function with a body are declared in source"),
            Local::LambdaParameter(parameter_id) => self
                .function
                .context()
                .assert_as_lambda_context()
                .get_parameter(parameter_id)
                .span(),
            Local::OperationHandlerParameter(parameter_id) => self
                .function
                .context()
                .assert_as_operation_handler_context()
                .get_parameter(parameter_id)
                .span(),
            Local::Capture(capture_id) => self
                .captures
                .expect("capture address roots require a nested function capture layout")
                .get_capture(capture_id)
                .span(),
        }
    }

    /// Returns the stack roots whose lifetime ends with `scope_id`, in the
    /// order their values are dropped.
    ///
    /// Values are dropped in reverse of the order they came into scope. Local
    /// variables drop in reverse declaration order. The root scope then drops
    /// the function inputs, as [`Self::push_inputs_in_drop_order`] orders them.
    pub(crate) async fn scope_roots_in_drop_order(&self, scope_id: ScopeID) -> Vec<Local> {
        let mut roots =
            self.function.declared_variables(scope_id).map(Local::Variable).collect::<Vec<_>>();
        roots.reverse();

        if self.function.root_scope_id() == scope_id {
            self.push_inputs_in_drop_order(&mut roots).await;
        }

        roots
    }

    /// Returns every stack root of the function in the order their values are
    /// dropped: local variables in reverse declaration order, then the
    /// function inputs, as [`Self::push_inputs_in_drop_order`] orders them.
    pub(crate) async fn roots_in_drop_order(&self) -> Vec<Local> {
        let mut variables = self
            .function
            .variables()
            .map(|(variable_id, variable)| (variable.declaration_order(), variable_id))
            .collect::<Vec<_>>();
        variables.sort_unstable_by_key(|(declaration_order, _)| Reverse(*declaration_order));

        let mut roots = variables
            .into_iter()
            .map(|(_, variable_id)| Local::Variable(variable_id))
            .collect::<Vec<_>>();
        self.push_inputs_in_drop_order(&mut roots).await;
        roots
    }

    /// Appends the function inputs in the order their values are dropped when
    /// the root scope ends: parameters in reverse, followed by captures in
    /// reverse, since the capture environment precedes the parameters.
    ///
    /// An operation handler only borrows its captures, which the enclosing
    /// function drops after the handled body, so they are not included.
    async fn push_inputs_in_drop_order(&self, roots: &mut Vec<Local>) {
        let parameters_start = roots.len();
        self.push_parameter_roots(roots).await;
        roots[parameters_start..].reverse();

        if !self.borrows_captures() {
            let captures_start = roots.len();
            roots.extend(self.capture_roots());
            roots[captures_start..].reverse();
        }
    }

    /// Returns whether `address` is rooted in a capture which this function
    /// only borrows, so no value may be moved out of it.
    ///
    /// Operation handlers may run many times over one shared environment, so
    /// every call must find its captures intact.
    pub(crate) fn is_borrowed_capture(&self, address: &Address) -> bool {
        self.borrows_captures() && matches!(tracked_local(address), Some(Local::Capture(_)))
    }

    /// Returns whether `load`, producing a value of type `ty`, moves out of
    /// its place: a forced move always does, and an implicit load does unless
    /// the value is `Copy`.
    ///
    /// A load through a raw pointer dereference is a bitwise copy of memory
    /// the frame does not own, so it never moves.
    pub(crate) async fn load_moves(&mut self, load: &Load, ty: Interned<Ty>) -> bool {
        if load.address().is_behind_deref() {
            return false;
        }

        match load.kind() {
            LoadKind::Implicit => !self.type_is_copy(ty).await,
            LoadKind::Move | LoadKind::Drop => true,
        }
    }

    /// Returns whether the function borrows its captures rather than owning
    /// them.
    const fn borrows_captures(&self) -> bool {
        match self.function.context() {
            IRContext::OperationHandler(_) => true,
            IRContext::Def | IRContext::Lambda(_) | IRContext::Thunk(_) => false,
        }
    }

    pub(crate) const fn solver_mut(&mut self) -> &mut Solver { &mut self.solver }

    pub(crate) const fn engine(&self) -> &rayc_qbice::TrackedEngine { self.solver.engine() }

    async fn type_is_copy(&mut self, ty: Interned<Ty>) -> bool {
        let marker_id = self.solver.engine().get_core_item(CoreItem::Copy).await;
        self.solver.entails_marker_predicate(MarkerPredicate::new(marker_id, ty)).await
    }

    /// Returns every function input: its parameters in declaration order,
    /// followed by its captures in capture-layout order.
    async fn input_roots(&self) -> Vec<Local> {
        let mut roots = Vec::new();
        self.push_parameter_roots(&mut roots).await;
        roots.extend(self.capture_roots());
        roots
    }

    /// Appends the function's parameters, in declaration order.
    async fn push_parameter_roots(&self, roots: &mut Vec<Local>) {
        match self.function.context() {
            IRContext::Def => {
                let parameters = self.solver.engine().get_parameter_map(self.solver.site()).await;
                roots.extend(
                    parameters.iter().map(|(parameter_id, _)| Local::Parameter(parameter_id)),
                );
            }
            IRContext::Lambda(context) => roots.extend(
                context.parameters().map(|(parameter_id, _)| Local::LambdaParameter(parameter_id)),
            ),
            IRContext::Thunk(_) => {}
            IRContext::OperationHandler(context) => roots.extend(
                context
                    .parameters()
                    .map(|(parameter_id, _)| Local::OperationHandlerParameter(parameter_id)),
            ),
        }
    }

    /// Returns the function's captures, in capture-layout order. Only nested
    /// functions have captures.
    fn capture_roots(&self) -> impl Iterator<Item = Local> + '_ {
        self.captures
            .into_iter()
            .flat_map(|captures| captures.iter().map(|(capture_id, _)| Local::Capture(capture_id)))
    }

    /// Returns the type of the component of `ty` selected by `projection`.
    ///
    /// # Panics
    ///
    /// Panics under the same conditions as [`Self::projection_layer`].
    pub(crate) async fn projected_type(
        &self,
        ty: &Interned<Ty>,
        projection: Projection,
    ) -> Interned<Ty> {
        self.projection_layer(ty, projection, None).await.1
    }

    /// Resolves one projection into its component states and selected type.
    ///
    /// [`PlaceState::move_at`] calls this immediately before traversing each
    /// projection. When the traversed place is uniform, its state is passed as
    /// `uniform`, and this directly constructs the component map used to
    /// expand it, avoiding an intermediate collection of sibling projections.
    /// Without a uniform state, no component map is built. The selected type
    /// becomes the input to the next projection.
    ///
    /// The type is normalized before its shape is inspected. Struct bodies and
    /// generic field substitutions are queried only for the current
    /// projection. Types beneath fields not selected by the address are never
    /// traversed, and no collection of future layers is built, which keeps
    /// recursive types finite.
    ///
    /// # Panics
    ///
    /// Panics if the normalized type is not an application, the projection
    /// kind does not match the type, or the requested component does not exist.
    pub(crate) async fn projection_layer(
        &self,
        ty: &Interned<Ty>,
        projection: Projection,
        uniform: Option<&PossibleStates>,
    ) -> (Option<BTreeMap<Projection, PlaceState>>, Interned<Ty>) {
        let ty = self.solver.normalize(ty).await;
        let Ty::Application(application) = &*ty else {
            panic!("projected type must normalize to an application: {ty:?}");
        };

        match (application.view(), projection) {
            (_, Projection::RawDeref) => {
                panic!("tracked places never extend past a dereference: {ty:?}");
            }
            (ApplicationView::Tuple(tuple), Projection::Tuple(index)) => {
                let projected = tuple
                    .args()
                    .get(index)
                    .unwrap_or_else(|| panic!("tuple projection index {index} is out of bounds"))
                    .clone();
                let components = uniform.map(|state| {
                    (0..tuple.args().len())
                        .map(|index| (Projection::Tuple(index), PlaceState::Uniform(state.clone())))
                        .collect()
                });
                (components, projected)
            }
            (ApplicationView::Struct(struct_ty), Projection::Field(field_id)) => {
                let engine = self.solver.engine();
                let substitution = struct_ty.create_subst(engine).await;
                let body = engine.get_struct_body(struct_ty.symbol_id()).await;
                let field = body
                    .iter()
                    .find_map(|(id, field)| (id == field_id).then_some(field))
                    .unwrap_or_else(|| {
                        panic!("struct projection references missing field {field_id:?}")
                    });
                let projected = field.ty().apply_subst_or_clone(&substitution, engine);
                let components = uniform.map(|state| {
                    body.iter()
                        .map(|(id, _)| (Projection::Field(id), PlaceState::Uniform(state.clone())))
                        .collect()
                });
                (components, projected)
            }
            (
                ApplicationView::Primitive(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Instance(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Error,
                Projection::Tuple(_) | Projection::Field(_),
            )
            | (ApplicationView::Tuple(_), Projection::Field(_))
            | (ApplicationView::Struct(_), Projection::Tuple(_)) => {
                panic!("projection {projection:?} does not match normalized type {ty:?}");
            }
        }
    }
}

impl JoinLattice<StackStateProblem<'_>, Infallible> for StackState {
    #[allow(clippy::match_same_arms)]
    async fn join(
        &mut self,
        other: &Self,
        _dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> Result<bool, Infallible> {
        match (&mut *self, other) {
            (Self::Unreachable, Self::Unreachable) => Ok(false),
            (Self::Reachable(_), Self::Unreachable) => Ok(false),

            (current @ Self::Unreachable, Self::Reachable(_)) => {
                *current = other.clone();
                Ok(true)
            }
            (Self::Reachable(current), Self::Reachable(incoming)) => {
                Ok(current.join_in_place(incoming))
            }
        }
    }
}

impl DataflowProblem for StackStateProblem<'_> {
    type JoinLattice = StackState;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<StackState, Infallible> {
        Ok(StackState::Unreachable)
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<StackState, Infallible> {
        Ok(StackState::reachable())
    }

    async fn transfer_instruction(
        &mut self,
        point: Point,
        instruction: &Instruction,
        state: &mut StackState,
    ) -> Result<(), Infallible> {
        let StackState::Reachable(slots) = state else {
            return Ok(());
        };

        match instruction {
            Instruction::ScopePush(scope_id) => {
                // Local variables begin their lifetime uninitialized.
                for variable_id in self.function.declared_variables(*scope_id) {
                    slots.set(Local::Variable(variable_id), PlaceState::uninitialized());
                }

                // Function inputs live for the root scope and arrive initialized.
                if self.function.root_scope_id() == *scope_id {
                    for root in self.input_roots().await {
                        slots.set(root, PlaceState::initialized());
                    }
                }
            }
            Instruction::ScopePop(scope_id) => {
                // Drop local slots as soon as their scope ends to keep states small.
                for variable_id in self.function.declared_variables(*scope_id) {
                    slots.remove(Local::Variable(variable_id));
                }

                // Function inputs share the root scope's lifetime.
                if self.function.root_scope_id() == *scope_id {
                    for root in self.input_roots().await {
                        slots.remove(root);
                    }
                }
            }
            Instruction::Expression(expression_id) => {
                let expression = self.function.get_expression(*expression_id);

                // Loads consume non-Copy places, and forced moves consume any
                // place. Other expressions do not directly change stack
                // initialization state.
                //
                // Moving out of a borrowed capture is an error reported by the
                // check, so the capture stays initialized to avoid cascading
                // use-after-move errors.
                if let IRExprKind::Load(load) = expression.kind()
                    && self.load_moves(load, expression.ty().clone()).await
                    && !self.is_borrowed_capture(load.address())
                {
                    let _ = state.move_out(load.address(), point, self).await;
                }
            }
            Instruction::ExprDiscard(_) => {}
            Instruction::Store(store) => {
                let address = store.address().clone();
                let _ = state.restore(&address, self).await;
            }
        }

        Ok(())
    }

    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut StackState,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut StackState,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}
