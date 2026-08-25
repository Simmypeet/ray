use std::io::Write;

use rayc_ir::ir_expr::make_lambda::MakeLambda;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{c_ty::CTy, context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&MakeLambda> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&MakeLambda>,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let lambda = expression.node();
        let target_context = function.target_lambda_context(lambda.function_id());
        assert_eq!(
            lambda.captures().len(),
            target_context.captures().len(),
            "compiler-internal invariant violation: MakeLambda capture operands do not match its \
             target environment"
        );

        let expression_type = function.instantiate_expression_type(expression.id(), ctx);
        let c_type = ctx.ty_to_cty(&expression_type);
        match &*c_type {
            CTy::Lambda(_) => {}
            CTy::Primitive(_) | CTy::Tuple(_) | CTy::Pointer(_) => panic!(
                "compiler-internal invariant violation: MakeLambda expression has a non-lambda C \
                 type"
            ),
        }

        write!(self, "(")?;
        ctx.write_cty(&c_type, self)?;
        write!(self, "){{ .{} = ", Identifier::lambda_call_field())?;
        let target = function.target_lambda(lambda.function_id());
        ctx.write_mono_function_name(&target, self).await?;
        write!(self, ", .{} = ", Identifier::lambda_env_field())?;
        if lambda.captures().is_empty() {
            write!(self, "(void *)0")?;
        } else {
            write!(self, "&{}", Identifier::lambda_environment_value(expression.id()))?;
        }
        write!(self, " }}")
    }
}

impl Writer<'_> {
    pub(crate) async fn write_lambda_environment_assignment(
        &mut self,
        expression: ExpressionWithID<&MakeLambda>,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let lambda = expression.node();
        let target_context = function.target_lambda_context(lambda.function_id());
        assert_eq!(
            lambda.captures().len(),
            target_context.captures().len(),
            "compiler-internal invariant violation: MakeLambda capture operands do not match its \
             target environment"
        );
        assert!(
            !lambda.captures().is_empty(),
            "compiler-internal invariant violation: captureless lambda has an environment \
             assignment"
        );

        let target = function.target_lambda(lambda.function_id());
        self.write_indent_line(async |writer| {
            write!(writer, "{} = (struct ", Identifier::lambda_environment_value(expression.id()))?;
            ctx.write_lambda_environment_name(&target, writer).await?;
            write!(writer, "){{ ")?;

            for (index, ((capture_id, _), operand)) in
                target_context.captures().zip(lambda.captures()).enumerate()
            {
                if index != 0 {
                    write!(writer, ", ")?;
                }
                write!(
                    writer,
                    ".{} = {}",
                    Identifier::capture_field(capture_id),
                    Identifier::expr(*operand)
                )?;
            }

            write!(writer, " }};")
        })
        .await
    }
}
