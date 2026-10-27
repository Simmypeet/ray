//! Computes the saved locals of a coroutine body from its IR.

use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_ir::{
    address::Local,
    get_ir,
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
    suspension_point::{SuspensionPoint, Waitee},
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map,
    saved_local::{SavedLocal, SavedLocalKind, SavedLocalOrigin, SavedLocals},
};
use rayc_symbol::{GlobalSymbolID, symbol_kind::get_symbol_kind};
use rayc_type::ty::Ty;

/// Collects the saved locals of a function and of the nested bodies of the
/// `run ... with`s it waits for.
struct Collector<'a> {
    engine: &'a TrackedEngine,
    functions: &'a IRFunctionMap,
    def_id: GlobalSymbolID,

    /// The spans of the `run ... with`s whose nested bodies are being
    /// collected, outermost first.
    runs: Vec<RelativeSpan>,

    saved_locals: Vec<SavedLocal>,
}

impl Collector<'_> {
    /// Collects what `function_id` holds at each of its suspension points.
    async fn collect(&mut self, function_id: IRFunctionID) {
        Box::pin(async move {
            let function = self.functions.get_function(function_id);

            for suspension_point in SuspensionPoint::collect(function).await {
                let suspension_span = function.get_expression(suspension_point.expression()).span();

                for &local in
                    suspension_point.held_locals().iter().chain(suspension_point.borrowed())
                {
                    let (ty, kind, span) = self.binding(function, local).await;
                    self.push(ty, kind, span, suspension_span).await;
                }

                for &value in suspension_point.held_expressions() {
                    let expression = function.get_expression(value);
                    self.push(
                        expression.ty().clone(),
                        SavedLocalKind::Value,
                        expression.span(),
                        suspension_span,
                    )
                    .await;
                }

                self.collect_waited(function, suspension_point.waited(), suspension_span).await;
            }
        })
        .await;
    }

    /// Collects what the expression of a suspension point at
    /// `suspension_span` waits for.
    async fn collect_waited(
        &mut self,
        function: &IRFunction,
        waited: Waitee<'_>,
        suspension_span: RelativeSpan,
    ) {
        match waited {
            // The arguments of a `perform` leave with the operation.
            Waitee::Perform => {}

            // The arguments of a call are the callee's upvars, so they are in
            // its coroutine type. An `extern def` has no body to wait in.
            Waitee::Call(call) => {
                if let Some(coroutine) = call.coroutine_ty(self.engine).await {
                    self.push(coroutine, SavedLocalKind::Callee, suspension_span, suspension_span)
                        .await;
                }
            }

            // The captures are the environment of the nested bodies, which
            // hold their own saved locals.
            Waitee::RunWith(run_with) => {
                for &capture in run_with.captures().iter().chain(run_with.handler_captures()) {
                    let expression = function.get_expression(capture);
                    self.push(
                        expression.ty().clone(),
                        SavedLocalKind::RunCapture,
                        expression.span(),
                        suspension_span,
                    )
                    .await;
                }

                self.runs.push(suspension_span);
                self.collect(run_with.body().function_id()).await;
                for handler in run_with.handlers() {
                    self.collect(handler.function_id()).await;
                }
                self.runs.pop();
            }

            Waitee::Handle(continuation) => {
                let expression = function.get_expression(continuation.continuation());
                self.push(
                    expression.ty().clone(),
                    SavedLocalKind::Continuation,
                    expression.span(),
                    suspension_span,
                )
                .await;
            }
        }
    }

    /// Returns the type, the kind and the declaration span of `local`, which
    /// is not a capture.
    async fn binding(
        &self,
        function: &IRFunction,
        local: Local,
    ) -> (Interned<Ty>, SavedLocalKind, RelativeSpan) {
        match local {
            Local::Variable(variable_id) => {
                let variable = function.get_variable(variable_id);
                let kind = if variable.is_temporary() {
                    SavedLocalKind::Temporary
                } else {
                    SavedLocalKind::Variable
                };
                (variable.ty().clone(), kind, variable.span())
            }
            Local::Parameter(parameter_id) => {
                let parameters = self.engine.get_parameter_map(self.def_id).await;
                let parameter = &parameters[parameter_id];
                let span =
                    parameter.span().expect("parameters of a function with a body are declared");
                (parameter.ty().clone(), SavedLocalKind::Parameter, span)
            }
            Local::ClosureParameter(parameter_id) => {
                let parameter =
                    function.context().assert_as_closure_context().get_parameter(parameter_id);
                (parameter.ty().clone(), SavedLocalKind::Parameter, parameter.span())
            }
            Local::OperationHandlerParameter(parameter_id) => {
                let parameter = function
                    .context()
                    .assert_as_operation_handler_context()
                    .get_parameter(parameter_id);
                (parameter.ty().clone(), SavedLocalKind::Parameter, parameter.span())
            }
            Local::Capture(_) => unreachable!("captures are never saved locals of their body"),
        }
    }

    async fn push(
        &mut self,
        ty: Interned<Ty>,
        kind: SavedLocalKind,
        span: RelativeSpan,
        suspension_span: RelativeSpan,
    ) {
        let ty = Ty::erase_lifetimes(&ty, self.engine).await;
        let origin = SavedLocalOrigin::new(kind, span, suspension_span, self.runs.clone());
        self.saved_locals.push(SavedLocal::new(ty, origin));
    }
}

#[executor(config = Config)]
async fn saved_locals_executor(
    &SavedLocals { body }: &SavedLocals,
    engine: &TrackedEngine,
) -> Option<Interned<[SavedLocal]>> {
    let def_id = body.def_id();

    // An `extern def` has no body, so nothing is ever suspended in it.
    if !engine.get_symbol_kind(def_id).await.has_def_body() {
        return Some(engine.intern_unsized([]));
    }

    // IR with errors may lack its drops, which would leave out the locals
    // that are only dropped after a suspension point.
    let functions = engine.get_ir(def_id).await;
    if !functions.no_error() {
        return None;
    }

    let mut collector = Collector {
        engine,
        functions: &functions,
        def_id,
        runs: Vec::new(),
        saved_locals: Vec::new(),
    };

    let function_id =
        body.closure_id().map_or(functions.root_id(), |x| functions.get_function_id_of_closure(x));

    collector.collect(function_id).await;

    Some(engine.intern_unsized(collector.saved_locals))
}

#[distributed_slice(RAY_PROGRAM)]
static SAVED_LOCALS_EXECUTOR: Registration<Config> =
    Registration::new::<SavedLocals, SavedLocalsExecutor>();
