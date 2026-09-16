use rayc_typed_ast::typed_expr::tuple_index::TupleIndex;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a TupleIndex>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a TupleIndex>,
    ) -> LoweredExpression {
        let tuple_index = expression.node();
        let operand_id = tuple_index.operand();
        let operand = self.lower_by_id(context, operand_id);
        let mut address = self.lower_to_address_or_temporary(context, operand_id, operand);
        self.project_tuple(&mut address, tuple_index.index());
        LoweredExpression::LValue(address)
    }
}
