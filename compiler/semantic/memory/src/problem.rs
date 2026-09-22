use std::{collections::BTreeMap, convert::Infallible};

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Projection,
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, Direction, JoinLattice},
    ir_expr::IRExprKind,
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureMap,
};
use rayc_semantic_element::{parameter::get_parameter_map, struct_body::get_struct_body};
use rayc_solver::Solver;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{
    subst::Substitutable,
    ty::{Ty, application::View as ApplicationView},
    where_clause::MarkerPredicate,
};

use crate::{PlaceState, StackRoot, StackState};

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

    pub(crate) async fn binding_type(&self, root: StackRoot) -> Interned<Ty> {
        match root {
            StackRoot::Variable(variable_id) => {
                self.function.get_variable(variable_id).ty().clone()
            }
            StackRoot::Parameter(parameter_id) => self
                .solver
                .engine()
                .get_parameter_map(self.solver.site())
                .await
                .iter()
                .find_map(|(id, parameter)| (id == parameter_id).then(|| parameter.ty().clone()))
                .expect("parameter address root must exist in the function signature"),
            StackRoot::LambdaParameter(parameter_id) => self
                .function
                .context()
                .assert_as_lambda_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            StackRoot::OperationHandlerParameter(parameter_id) => self
                .function
                .context()
                .assert_as_operation_handler_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            StackRoot::Capture(capture_id) => self
                .captures
                .expect("capture address roots require a nested function capture layout")
                .get_capture(capture_id)
                .storage_ty(self.solver.engine()),
        }
    }

    async fn type_is_copy(&mut self, ty: Interned<Ty>) -> bool {
        let marker_id = self.solver.engine().get_core_item(CoreItem::Copy).await;
        self.solver.entails_marker_predicate(MarkerPredicate::new(marker_id, ty)).await
    }

    async fn root_stack_roots(&self) -> Vec<StackRoot> {
        let mut roots = match self.function.context() {
            IRContext::Def => self
                .solver
                .engine()
                .get_parameter_map(self.solver.site())
                .await
                .iter()
                .map(|(parameter_id, _)| StackRoot::Parameter(parameter_id))
                .collect(),
            IRContext::Lambda(context) => context
                .parameters()
                .map(|(parameter_id, _)| StackRoot::LambdaParameter(parameter_id))
                .collect(),
            IRContext::Thunk(_) => Vec::new(),
            IRContext::OperationHandler(context) => context
                .parameters()
                .map(|(parameter_id, _)| StackRoot::OperationHandlerParameter(parameter_id))
                .collect(),
        };

        // Every nested function context also owns its captured stack inputs.
        match self.function.context() {
            IRContext::Def => {}
            IRContext::Lambda(_) | IRContext::Thunk(_) | IRContext::OperationHandler(_) => {
                roots.extend(
                    self.captures
                        .expect("nested functions require a capture layout")
                        .iter()
                        .map(|(capture_id, _)| StackRoot::Capture(capture_id)),
                );
            }
        }
        roots
    }

    /// Resolves one projection into its component states and selected type.
    ///
    /// [`PlaceState::move_at`] calls this immediately before traversing each
    /// projection. When `place_state` is uniform, this directly constructs the
    /// component map used to expand it, avoiding an intermediate collection of
    /// sibling projections. An already-partial place returns no replacement
    /// map. The selected type becomes the input to the next projection.
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
        place_state: &PlaceState,
    ) -> (Option<BTreeMap<Projection, PlaceState>>, Interned<Ty>) {
        let ty = self.solver.normalize(ty).await;
        let Ty::Application(application) = &*ty else {
            panic!("projected type must normalize to an application: {ty:?}");
        };

        match (application.view(), projection) {
            (ApplicationView::Tuple(tuple), Projection::Tuple(index)) => {
                let projected = tuple
                    .args()
                    .get(index)
                    .unwrap_or_else(|| panic!("tuple projection index {index} is out of bounds"))
                    .clone();
                let components = match place_state {
                    PlaceState::Uniform(state) => Some(
                        (0..tuple.args().len())
                            .map(|index| {
                                (Projection::Tuple(index), PlaceState::Uniform(state.clone()))
                            })
                            .collect(),
                    ),
                    PlaceState::Partial(_) => None,
                };
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
                let components = match place_state {
                    PlaceState::Uniform(state) => Some(
                        body.iter()
                            .map(|(id, _)| {
                                (Projection::Field(id), PlaceState::Uniform(state.clone()))
                            })
                            .collect(),
                    ),
                    PlaceState::Partial(_) => None,
                };
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
                    slots.set(StackRoot::Variable(variable_id), PlaceState::uninitialized());
                }

                // Function inputs live for the root scope and arrive initialized.
                if self.function.root_scope_id() == *scope_id {
                    for root in self.root_stack_roots().await {
                        slots.set(root, PlaceState::initialized());
                    }
                }
            }
            Instruction::ScopePop(scope_id) => {
                // Drop local slots as soon as their scope ends to keep states small.
                for variable_id in self.function.declared_variables(*scope_id) {
                    slots.remove(StackRoot::Variable(variable_id));
                }

                // Function inputs share the root scope's lifetime.
                if self.function.root_scope_id() == *scope_id {
                    for root in self.root_stack_roots().await {
                        slots.remove(root);
                    }
                }
            }
            Instruction::Expression(expression_id) => {
                let expression = self.function.get_expression(*expression_id);

                // Loads consume non-Copy places. Other expressions do not directly
                // change stack initialization state.
                if let IRExprKind::Load(load) = expression.kind() {
                    let address = load.address().clone();
                    let ty = expression.ty().clone();
                    if !self.type_is_copy(ty).await {
                        let _ = state.move_out(&address, point, self).await;
                    }
                }
            }
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
