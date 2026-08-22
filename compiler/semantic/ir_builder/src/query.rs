use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_ir::function::Function;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_typed_ast::get_typed_ast;

use crate::lower_function;

#[executor(config = Config)]
async fn ir_executor(
    &rayc_ir::Key { def_id }: &rayc_ir::Key,
    engine: &TrackedEngine,
) -> Interned<Function> {
    let typed_function = engine.get_typed_ast(def_id).await;
    let function = lower_function(engine, &typed_function);
    engine.intern(function)
}

#[distributed_slice(RAY_PROGRAM)]
static IR_EXECUTOR: Registration<Config> = Registration::new::<rayc_ir::Key, IrExecutor>();
