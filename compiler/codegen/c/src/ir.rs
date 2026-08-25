use std::io::Write;

use rayc_hash::FxHashSet;
use rayc_ir::{
    cfg::{BlockID, Instruction, Reachables, Terminator},
    ir_expr::{ExpressionID, IRExprKind},
    ir_function::IRFunction,
    ir_variable::IRVariableID,
};
use rayc_mono::MonoFunction;

use crate::{
    context::Context,
    expression::function_instance::FunctionInstance,
    identifier::Identifier,
    writer::{EnclosingPair, Writer},
};

#[derive(Debug)]
struct FunctionLayout {
    reachables: Reachables,
    variables: Vec<IRVariableID>,
    phis: FxHashSet<ExpressionID>,
}

impl FunctionLayout {
    fn new(function: &IRFunction) -> Self {
        let reachables = function.reachables();
        let phis = reachables
            .expressions()
            .filter(|expression_id| {
                matches!(function.get_expression(*expression_id).kind(), IRExprKind::Phi(_))
            })
            .collect();

        let variables: Vec<_> = function.variables().map(|(id, _)| id).collect();

        Self { reachables, variables, phis }
    }

    fn reachable_blocks(&self) -> impl Iterator<Item = BlockID> + '_ { self.reachables.blocks() }

    fn reachable_expressions(&self) -> impl Iterator<Item = ExpressionID> + '_ {
        self.reachables.expressions()
    }

    fn variables(&self) -> impl Iterator<Item = IRVariableID> + '_ {
        self.variables.iter().copied()
    }

    fn is_phi(&self, expression_id: ExpressionID) -> bool { self.phis.contains(&expression_id) }
}

impl Writer<'_> {
    async fn generate_unit_value(&mut self, ctx: &Context) -> std::io::Result<()> {
        let unit_id = ctx.get_unit_tuple_id();

        self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            writer
                .write_enclosing_pair(EnclosingPair::Parens, async |writer| {
                    ctx.write_ctuple_t(unit_id, writer)
                })
                .await?;
            writer
                .write_enclosing_pair(EnclosingPair::Braces, async |writer| write!(writer, "0"))
                .await
        })
        .await
    }

    pub(crate) async fn write_ir_function_body(
        &mut self,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let ir_function = function.function();
        let mono_function = function.mono_function();
        let layout = FunctionLayout::new(ir_function);

        self.write_braced_block(async |writer| {
            if function.is_lambda() {
                let lambda = function.lambda_context();
                if lambda.captures().next().is_some() {
                    writer
                        .write_indent_line(async |writer| {
                            write!(writer, "struct ")?;
                            ctx.write_lambda_environment_name(mono_function, writer).await?;
                            write!(
                                writer,
                                " *{} = {};",
                                Identifier::lambda_typed_environment(),
                                Identifier::lambda_raw_environment()
                            )
                        })
                        .await?;
                }
            }

            for variable_id in layout.variables() {
                writer
                    .write_indent_line(async |writer| {
                        let ty = ctx.instantiate_type(
                            ir_function.get_variable(variable_id).ty(),
                            mono_function,
                        );
                        let cty = ctx.ty_to_cty(&ty);
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " {};", Identifier::var(variable_id))
                    })
                    .await?;
            }

            for expression_id in layout.reachable_expressions() {
                writer
                    .write_indent_line(async |writer| {
                        let ty = ctx.instantiate_type(
                            ir_function.get_expression(expression_id).ty(),
                            mono_function,
                        );
                        let cty = ctx.ty_to_cty(&ty);
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " {};", Identifier::expr(expression_id))
                    })
                    .await?;
            }

            // Capture environments are function-scope stack objects. This is sound for the
            // current non-escaping closure subset; semantic lifetime checking must reject a
            // captureful closure that outlives this invocation.
            for expression_id in layout.reachable_expressions() {
                let IRExprKind::MakeLambda(lambda) =
                    ir_function.get_expression(expression_id).kind()
                else {
                    continue;
                };
                if lambda.captures().is_empty() {
                    continue;
                }
                let target = function.target_lambda(lambda.function_id());
                let _ = function.target_lambda_context(lambda.function_id());

                writer
                    .write_indent_line(async |writer| {
                        write!(writer, "struct ")?;
                        ctx.write_lambda_environment_name(&target, writer).await?;
                        write!(writer, " {};", Identifier::lambda_environment_value(expression_id))
                    })
                    .await?;
            }

            for block_id in layout.reachable_blocks() {
                writer
                    .write_outdented_line(async |writer| {
                        write!(writer, "{}:", Identifier::block(block_id))
                    })
                    .await?;

                for instruction in ir_function.block_instructions(block_id) {
                    writer.write_ir_instruction(instruction, function, &layout, ctx).await?;
                }

                writer
                    .write_ir_terminator(
                        block_id,
                        ir_function
                            .block_terminator(block_id)
                            .expect("reachable IR block should have a terminator"),
                        ir_function,
                        mono_function,
                        ctx,
                    )
                    .await?;
            }

            Ok(())
        })
        .await
    }

    async fn write_ir_instruction(
        &mut self,
        instruction: &Instruction,
        function: FunctionInstance<'_>,
        layout: &FunctionLayout,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let ir_function = function.function();
        match instruction {
            Instruction::Expression(expression_id) => {
                if layout.is_phi(*expression_id) {
                    return Ok(());
                }

                if let IRExprKind::MakeLambda(lambda) =
                    ir_function.get_expression(*expression_id).kind()
                    && !lambda.captures().is_empty()
                {
                    self.write_lambda_environment_assignment(
                        crate::expression::ExpressionWithID::new(lambda, *expression_id),
                        function,
                        ctx,
                    )
                    .await?;
                }

                self.write_indent_line(async |writer| {
                    write!(writer, "{} = ", Identifier::expr(*expression_id))?;
                    writer.write_expression_value(*expression_id, function, ctx).await?;
                    write!(writer, ";")
                })
                .await
            }
            Instruction::Store(store) => {
                self.write_indent_line(async |writer| {
                    writer.write_address(store.address(), function)?;
                    write!(writer, " = {};", Identifier::expr(store.expression()))
                })
                .await
            }
        }
    }

    async fn write_ir_terminator(
        &mut self,
        block_id: BlockID,
        terminator: &Terminator,
        function: &IRFunction,
        mono_function: &MonoFunction,
        ctx: &Context,
    ) -> std::io::Result<()> {
        match terminator {
            Terminator::Jump(successor) => {
                if Self::block_has_phis(*successor, function) {
                    self.write_indent_line(async |writer| {
                        writer
                            .write_braced_block(async |writer| {
                                writer
                                    .write_ir_edge(
                                        block_id,
                                        *successor,
                                        function,
                                        mono_function,
                                        ctx,
                                    )
                                    .await
                            })
                            .await
                    })
                    .await
                } else {
                    self.write_indent_line(async |writer| {
                        write!(writer, "goto {};", Identifier::block(*successor))
                    })
                    .await
                }
            }
            Terminator::Conditional(conditional) => {
                self.write_indent_line(async |writer| {
                    write!(writer, "if ({}) ", Identifier::expr(conditional.condition()))?;
                    writer
                        .write_braced_block(async |writer| {
                            writer
                                .write_ir_edge(
                                    block_id,
                                    conditional.then_block(),
                                    function,
                                    mono_function,
                                    ctx,
                                )
                                .await
                        })
                        .await?;
                    write!(writer, " else ")?;
                    writer
                        .write_braced_block(async |writer| {
                            writer
                                .write_ir_edge(
                                    block_id,
                                    conditional.else_block(),
                                    function,
                                    mono_function,
                                    ctx,
                                )
                                .await
                        })
                        .await
                })
                .await
            }
            Terminator::Return(value) => {
                self.write_indent_line(async |writer| {
                    write!(writer, "return ")?;
                    if let Some(expression_id) = value {
                        write!(writer, "{}", Identifier::expr(*expression_id))?;
                    } else {
                        writer.generate_unit_value(ctx).await?;
                    }
                    write!(writer, ";")
                })
                .await
            }
        }
    }

    fn block_has_phis(block_id: BlockID, function: &IRFunction) -> bool {
        function.block_instructions(block_id).iter().any(|instruction| {
            let Instruction::Expression(expression_id) = instruction else {
                return false;
            };
            matches!(function.get_expression(*expression_id).kind(), IRExprKind::Phi(_))
        })
    }

    async fn write_ir_edge(
        &mut self,
        predecessor: BlockID,
        successor: BlockID,
        function: &IRFunction,
        mono_function: &MonoFunction,
        ctx: &Context,
    ) -> std::io::Result<()> {
        for instruction in function.block_instructions(successor) {
            let Instruction::Expression(phi_id) = instruction else {
                continue;
            };
            let IRExprKind::Phi(phi) = function.get_expression(*phi_id).kind() else {
                continue;
            };
            let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                panic!(
                    "invalid IR reached C codegen: phi {} in block {} has no incoming value from \
                     predecessor {}",
                    phi_id.index(),
                    successor.index(),
                    predecessor.index()
                )
            });

            self.write_indent_line(async |writer| {
                let ty = ctx.instantiate_type(function.get_expression(*phi_id).ty(), mono_function);
                let cty = ctx.ty_to_cty(&ty);
                ctx.write_cty(&cty, writer)?;
                write!(
                    writer,
                    " {} = {};",
                    Identifier::phi_input(predecessor, successor, *phi_id),
                    Identifier::expr(incoming)
                )
            })
            .await?;
        }

        for instruction in function.block_instructions(successor) {
            let Instruction::Expression(phi_id) = instruction else {
                continue;
            };
            if !matches!(function.get_expression(*phi_id).kind(), IRExprKind::Phi(_)) {
                continue;
            }

            self.write_indent_line(async |writer| {
                write!(
                    writer,
                    "{} = {};",
                    Identifier::expr(*phi_id),
                    Identifier::phi_input(predecessor, successor, *phi_id)
                )
            })
            .await?;
        }

        self.write_indent_line(async |writer| {
            write!(writer, "goto {};", Identifier::block(successor))
        })
        .await
    }
}
