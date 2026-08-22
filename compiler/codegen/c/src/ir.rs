use std::io::Write;

use rayc_hash::FxHashSet;
use rayc_ir::{
    cfg::{BlockID, Instruction, Reachables, Terminator},
    expression::{ExpressionID, ExpressionKind},
    function::Function,
    variable::VariableID,
};

use crate::{
    context::Context,
    identifier::Identifier,
    writer::{EnclosingPair, Writer},
};

#[derive(Debug)]
struct FunctionLayout {
    reachables: Reachables,
    variables: Vec<VariableID>,
    phis: FxHashSet<ExpressionID>,
}

impl FunctionLayout {
    fn new(function: &Function) -> Self {
        let reachables = function.reachables();
        let phis = reachables
            .expressions()
            .filter(|expression_id| {
                matches!(function.get_expression(*expression_id).kind(), ExpressionKind::Phi(_))
            })
            .collect();

        let variables: Vec<_> = function.variables().map(|(id, _)| id).collect();

        Self { reachables, variables, phis }
    }

    fn reachable_blocks(&self) -> impl Iterator<Item = BlockID> + '_ { self.reachables.blocks() }

    fn reachable_expressions(&self) -> impl Iterator<Item = ExpressionID> + '_ {
        self.reachables.expressions()
    }

    fn variables(&self) -> impl Iterator<Item = VariableID> + '_ { self.variables.iter().copied() }

    fn is_phi(&self, expression_id: ExpressionID) -> bool { self.phis.contains(&expression_id) }
}

impl Writer<'_> {
    async fn generate_unit_value(&mut self, ctx: &mut Context) -> std::io::Result<()> {
        let unit_id = ctx.get_unit_ctuple_id();

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
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let layout = FunctionLayout::new(function);

        self.write_braced_block(async |writer| {
            for variable_id in layout.variables() {
                writer
                    .write_indent_line(async |writer| {
                        let cty = ctx.ty_to_cty(function.get_variable(variable_id).ty());
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " {};", Identifier::var(variable_id))
                    })
                    .await?;
            }

            for expression_id in layout.reachable_expressions() {
                writer
                    .write_indent_line(async |writer| {
                        let cty = ctx.ty_to_cty(function.get_expression(expression_id).ty());
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " {};", Identifier::expr(expression_id))
                    })
                    .await?;
            }

            for block_id in layout.reachable_blocks() {
                writer
                    .write_indent_line(async |writer| {
                        write!(writer, "{}:", Identifier::block(block_id))
                    })
                    .await?;

                for instruction in function.block_instructions(block_id) {
                    writer.write_ir_instruction(instruction, function, &layout, ctx).await?;
                }

                writer
                    .write_ir_terminator(
                        block_id,
                        function
                            .block_terminator(block_id)
                            .expect("reachable IR block should have a terminator"),
                        function,
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
        function: &Function,
        layout: &FunctionLayout,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        match instruction {
            Instruction::Expression(expression_id) => {
                if layout.is_phi(*expression_id) {
                    return Ok(());
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
                    writer.write_address(store.address())?;
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
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        match terminator {
            Terminator::Jump(successor) => {
                if Self::block_has_phis(*successor, function) {
                    self.write_indent_line(async |writer| {
                        writer
                            .write_braced_block(async |writer| {
                                writer.write_ir_edge(block_id, *successor, function, ctx).await
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
                                .write_ir_edge(block_id, conditional.then_block(), function, ctx)
                                .await
                        })
                        .await?;
                    write!(writer, " else ")?;
                    writer
                        .write_braced_block(async |writer| {
                            writer
                                .write_ir_edge(block_id, conditional.else_block(), function, ctx)
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

    fn block_has_phis(block_id: BlockID, function: &Function) -> bool {
        function.block_instructions(block_id).iter().any(|instruction| {
            let Instruction::Expression(expression_id) = instruction else {
                return false;
            };
            matches!(function.get_expression(*expression_id).kind(), ExpressionKind::Phi(_))
        })
    }

    async fn write_ir_edge(
        &mut self,
        predecessor: BlockID,
        successor: BlockID,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        for instruction in function.block_instructions(successor) {
            let Instruction::Expression(phi_id) = instruction else {
                continue;
            };
            let ExpressionKind::Phi(phi) = function.get_expression(*phi_id).kind() else {
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
                let cty = ctx.ty_to_cty(function.get_expression(*phi_id).ty());
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
            if !matches!(function.get_expression(*phi_id).kind(), ExpressionKind::Phi(_)) {
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
