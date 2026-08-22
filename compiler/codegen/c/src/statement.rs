use std::io::Write;

use rayc_typed_ast::statement::{Let, Return, Statement};

use crate::{
    context::Context,
    function_ctx::FunctionCtx,
    writer::{EnclosingPair, Writer},
};

impl Writer<'_> {
    pub async fn generate_statement(
        &mut self,
        statement: &Statement,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        self.write_indent_line(async |writer| {
            match statement {
                Statement::Let(let_statement) => {
                    writer.generate_let(let_statement, function_ctx, ctx).await?;
                }
                Statement::Expression(expression) => {
                    writer.generate_typed_expr(*expression, function_ctx, ctx).await?;
                }
                Statement::Return(return_statement) => {
                    writer.generate_return(return_statement, function_ctx, ctx).await?;
                }
            }

            write!(writer, ";")
        })
        .await
    }

    async fn generate_let(
        &mut self,
        let_statement: &Let,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let variable_ty = function_ctx.get_variable(let_statement.variable_id()).ty().clone();
        let cty = ctx.ty_to_cty(&variable_ty);

        ctx.write_cty(&cty, self)?;
        write!(self, " ray_var_{:X} = ", let_statement.variable_id().index())?;
        self.generate_typed_expr(let_statement.expression(), function_ctx, ctx).await
    }

    async fn generate_return(
        &mut self,
        return_statement: &Return,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        write!(self, "return ")?;

        if let Some(expression) = return_statement.value() {
            self.generate_typed_expr(expression, function_ctx, ctx).await
        } else {
            self.generate_unit_value(ctx).await
        }
    }

    pub(crate) async fn generate_unit_value(&mut self, ctx: &mut Context) -> std::io::Result<()> {
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
}
