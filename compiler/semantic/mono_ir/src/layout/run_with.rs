//! What a `coroutine_resume` holds while it is suspended at a `run ... with`.
//!
//! The handled body and the clauses run as coroutines of their own, which the
//! suspended function resumes. They all live in one aggregate at the
//! suspension point, so the frame keeps one state per suspension point, and
//! the aggregate tells by itself which of them runs.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_ir::{get_ir, ir_expr::IRExprKind};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::GlobalSymbolID;

use crate::{
    MonoCoroutineInstance, MonoEnvironmentInstance, MonoRunWithInstance, layout::TagType,
    operand::Constant,
};

/// The clause of one operation of a `run ... with`, which runs as a
/// coroutine.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct RunWithClause {
    operation_id: GlobalSymbolID,
    coroutine: MonoCoroutineInstance,
}

impl RunWithClause {
    /// The operation that the clause handles.
    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    /// The coroutine that runs the clause.
    #[must_use]
    pub const fn coroutine(&self) -> &MonoCoroutineInstance { &self.coroutine }
}

/// What runs while a function is suspended at a `run ... with`: the handled
/// body, or the clause of one operation, which the body waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWithPhase {
    Body,
    /// The clause of the operation runs.
    Clause(GlobalSymbolID),
}

/// The members of a [`AggregateType::RunWithFrames`].
///
/// At most one clause runs at a time, since the body waits at its operation
/// until the clause returns, so the frames of the clauses overlap.
///
/// [`AggregateType::RunWithFrames`]: crate::ty::AggregateType::RunWithFrames
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct RunWithLayout {
    instance: MonoRunWithInstance,
    handler_environment: Option<MonoEnvironmentInstance>,
    body: MonoCoroutineInstance,
    clauses: Vec<RunWithClause>,
}

impl RunWithLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoRunWithInstance { &self.instance }

    /// The environment that the clauses share, which only borrow it, if the
    /// `run ... with` has clauses.
    #[must_use]
    pub const fn handler_environment(&self) -> Option<&MonoEnvironmentInstance> {
        self.handler_environment.as_ref()
    }

    /// The coroutine that runs the handled body.
    #[must_use]
    pub const fn body(&self) -> &MonoCoroutineInstance { &self.body }

    /// The clauses, in the order the `run ... with` lists them.
    #[must_use]
    pub fn clauses(&self) -> impl ExactSizeIterator<Item = &RunWithClause> { self.clauses.iter() }

    /// The clause of `operation_id`.
    ///
    /// # Panics
    ///
    /// Panics if the `run ... with` has no clause for the operation.
    #[must_use]
    pub fn clause(&self, operation_id: GlobalSymbolID) -> &RunWithClause {
        self.clauses
            .iter()
            .find(|clause| clause.operation_id == operation_id)
            .expect("a `run ... with` should have a clause for every operation it handles")
    }

    /// The type of the phase, which tells the body and the clauses apart.
    #[must_use]
    pub fn phase_tag_type(&self) -> TagType { TagType::for_variant_count(1 + self.clauses.len()) }

    /// The tag of `phase`: `0` for the body, and one more than its position
    /// for a clause.
    #[must_use]
    pub fn phase_tag(&self, phase: RunWithPhase) -> Constant {
        let tag = match phase {
            RunWithPhase::Body => 0,
            RunWithPhase::Clause(operation_id) => {
                let position = self
                    .clauses
                    .iter()
                    .position(|clause| clause.operation_id == operation_id)
                    .expect("a `run ... with` should have a clause for every operation it handles");
                position + 1
            }
        };

        self.phase_tag_type().constant(u64::try_from(tag).expect("a tag should fit in u64"))
    }
}

/// Derives the members of what a function holds while it is suspended at a
/// `run ... with`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<RunWithLayout>)]
#[extend(name = build_run_with_layout, by_val)]
pub struct BuildRunWithLayout {
    /// The `run ... with` whose body and clauses determine the layout.
    pub instance: MonoRunWithInstance,
}

#[executor(config = Config)]
async fn build_run_with_layout_executor(
    key: &BuildRunWithLayout,
    engine: &TrackedEngine,
) -> Interned<RunWithLayout> {
    let instance = &key.instance;
    let owner = instance.owner();
    let source = engine.get_ir(owner.def_id()).await;
    let function = source.get_function(owner.ir_function_id());
    let IRExprKind::RunWith(run_with) = function.get_expression(instance.expression()).kind()
    else {
        panic!("the frames of a `run ... with` should be keyed by a `run ... with` expression")
    };

    // The body and the clauses are functions of the IR of the owner, in its
    // instantiation, each suspended as a coroutine of its own.
    let body = MonoCoroutineInstance::new(owner.with_ir_function(run_with.body().function_id()));
    let clauses = run_with
        .handlers()
        .iter()
        .map(|handler| RunWithClause {
            operation_id: handler.operation_id(),
            coroutine: MonoCoroutineInstance::new(owner.with_ir_function(handler.function_id())),
        })
        .collect();
    let handler_environment = run_with
        .handler_capture_map()
        .map(|capture_map| MonoEnvironmentInstance::new(owner, capture_map));

    engine.intern(RunWithLayout { instance: instance.clone(), handler_environment, body, clauses })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_RUN_WITH_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildRunWithLayout, BuildRunWithLayoutExecutor>();
