use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{LvalueClassification, TypedExprID, tuple_index::TupleIndex},
};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a TupleIndex>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a TupleIndex>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let tuple_index = expression.node();
        let expression_id = expression.id();
        match typed_function.classify_lvalue(expression_id) {
            LvalueClassification::Lvalue(_) => {
                emit_load(self, typed_function, expression_id, span, ty)
            }
            LvalueClassification::NotLvalue => {
                let operand_ty = typed_function.get_type_of_expr_id(tuple_index.operand()).clone();
                let operand_value =
                    self.lower_expression_by_id(typed_function, tuple_index.operand());
                let temporary = self.create_temporary(operand_ty, span);
                let mut address = self.variable_address(temporary);
                self.emit_store(address.clone(), operand_value);
                self.project_tuple(&mut address, tuple_index.index());
                self.emit_expression(Expression::new(
                    ExpressionKind::Load(Load::new(address)),
                    span,
                    ty,
                ))
            }
            LvalueClassification::Errored => self.emit_expression(Expression::new_error(span, ty)),
        }
    }
}

fn emit_load(
    builder: &mut Builder,
    typed_function: &TypedFunction,
    expression_id: TypedExprID,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> ExpressionID {
    let address = builder.lower_address_by_id(typed_function, expression_id);
    builder.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
}
