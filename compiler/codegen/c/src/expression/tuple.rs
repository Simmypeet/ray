use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, tuple::Tuple};

use crate::{
    context::Context,
    expr_ctx::ExprCtx,
    expression::Generate,
    writer::{EnclosingPair, Writer},
};

impl Generate<Tuple> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Tuple,
        expr_id: TypedExprID,
        expr_ctx: &ExprCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        // write the tuple value like `((RayTupleXYZ_t){ .elem0 = <expr>, ... })`
        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            // write the tuple type like `(RayTupleXYZ_t)`
            writer
                .write_enclosing_pair(EnclosingPair::Parens, async |writer| {
                    let tuple_ty = expr_ctx.get_type_of_expr_id(expr_id);
                    let ctuple_id = ctx.unwrap_ty_as_ctuple_id(tuple_ty);
                    ctx.write_ctuple_t(ctuple_id, writer)
                })
                .await?;

            // write the tuple elements like `{ .elem0 = <expr>, ... }`
            writer
                .write_separated_list(
                    EnclosingPair::Braces,
                    expr.elements().iter().enumerate(),
                    ',',
                    async |writer, (n, expr_id)| {
                        write!(writer, ".elem{n:X} = ")?;
                        writer.generate_typed_expr(*expr_id, expr_ctx, ctx).await
                    },
                )
                .await
        }))
        .await
    }
}
