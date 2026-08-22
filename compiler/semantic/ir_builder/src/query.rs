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
    function.validate().expect("queried IR should be structurally valid");
    engine.intern(function)
}

#[distributed_slice(RAY_PROGRAM)]
static IR_EXECUTOR: Registration<Config> = Registration::new::<rayc_ir::Key, IrExecutor>();

#[cfg(test)]
mod test {
    use std::{collections::HashMap, sync::Arc};

    use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
    use rayc_ir::{
        cfg::{Instruction, Terminator},
        expression::ExpressionKind,
        get_ir,
    };
    use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
    use rayc_source_file::LocalSourceID;
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;
    use rayc_type::ty::{Primitive, Ty};
    use rayc_typed_ast::{
        function::Function as TypedFunction,
        statement::{Return, Statement},
        typed_expr::{
            TypedExpr, TypedExprKind,
            binary::{Binary, BinaryOp},
            literal::Literal,
        },
    };

    use super::IrExecutor;

    fn span() -> RelativeSpan {
        let location = RelativeLocation {
            offset: 0,
            mode: OffsetMode::Start,
            relative_to: rayc_arena::ID::<Branch>::new(0),
        };
        RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
    }

    async fn logical_function() -> qbice::storage::intern::Interned<TypedFunction> {
        let engine = rayc_qbice::create_minimal_engine().await;
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let mut function = TypedFunction::default();
        let left = function.insert_expression(TypedExpr::new(
            TypedExprKind::Literal(Literal::Bool(true)),
            span(),
            bool_ty.clone(),
        ));
        let right = function.insert_expression(TypedExpr::new(
            TypedExprKind::Literal(Literal::Bool(false)),
            span(),
            bool_ty.clone(),
        ));
        let logical = function.insert_expression(TypedExpr::new(
            TypedExprKind::Binary(Binary::new(left, BinaryOp::And, right)),
            span(),
            bool_ty,
        ));
        function.push_statement(Statement::Return(Return::new_with_value(logical)));
        engine.intern(function)
    }

    #[tokio::test]
    async fn query_returns_cached_completed_logical_ir() {
        let def_id = TargetID::TEST.make_global(SymbolID::default());
        let typed_function = logical_function().await;
        let mut engine =
            Engine::new_with(Plugin::default(), InMemoryFactory, SeededStableHasherBuilder::new(0))
                .await
                .unwrap();
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            rayc_typed_ast::Key { def_id },
            typed_function,
        )]))));
        engine.register_executor(Arc::new(IrExecutor));
        let engine = Arc::new(engine).tracked().await;

        let first = engine.get_ir(def_id).await;
        let second = engine.get_ir(def_id).await;

        assert_eq!(first, second);
        assert!(std::ptr::eq(first.as_ref(), second.as_ref()));
        assert_eq!(first.validate(), Ok(()));

        let mut pending = vec![first.entry_block()];
        let mut visited = Vec::new();
        let mut found_phi = false;
        while let Some(block) = pending.pop() {
            if visited.contains(&block) {
                continue;
            }
            visited.push(block);
            for instruction in first.block_instructions(block) {
                if let Instruction::Expression(expression) = instruction {
                    found_phi |=
                        matches!(first.get_expression(*expression).kind(), ExpressionKind::Phi(_));
                }
            }
            match first.block_terminator(block).expect("queried blocks should be terminated") {
                Terminator::Jump(successor) => pending.push(*successor),
                Terminator::Conditional(conditional) => {
                    pending.push(conditional.then_block());
                    pending.push(conditional.else_block());
                }
                Terminator::Return(_) => {}
            }
        }

        assert_eq!(visited.len(), 4);
        assert!(found_phi);
    }
}
