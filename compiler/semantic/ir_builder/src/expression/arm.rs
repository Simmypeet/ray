use rayc_ir::ir_expr::IRExprID;
use rayc_lexical::tree::RelativeSpan;
use rayc_typed_ast::typed_expr::arm::Arm;

use crate::{builder::Builder, context::LoweringContext};

impl Builder {
    /// Lowers an arm, such as one of an `if` or the body of a `handle`
    /// clause, which gives the value of the whole expression at `span`.
    ///
    /// Returns `None` when control never leaves the arm through its end.
    pub(super) async fn lower_arm(
        &mut self,
        context: &LoweringContext<'_>,
        arm: &Arm,
        span: RelativeSpan,
    ) -> Option<IRExprID> {
        match arm {
            Arm::Expression(expression) => {
                Some(self.lower_rvalue_by_id(context, *expression).await)
            }
            Arm::Block(statements) => {
                self.lower_statement_list(context, statements).await;
                (!self.is_terminated()).then(|| self.emit_unit(span))
            }
        }
    }
}
