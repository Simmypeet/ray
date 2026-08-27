use rayc_source_file::SourceElement;
use rayc_syntax::expression::Parenthesized;
use rayc_type::ty::Ty;
use rayc_typed_ast::typed_expr::{
    TypedExpr, TypedExprID, TypedExprKind, paren::Paren, tuple::Tuple,
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<Parenthesized> for TAstBuilder {
    async fn bind(&mut self, syn: Parenthesized) -> TypedExprID {
        let mut args = Vec::new();

        for arg in syn.expressions() {
            args.push(self.bind(arg).await);
        }

        let tree = syn.inner_tree();
        let has_comma = tree
            .nodes()
            .iter()
            .any(|x| x.as_leaf().and_then(|x| x.kind.as_punctuation()).is_some_and(|x| x.0 == ','));

        // if contains only one argument and has no comma then it is simply a
        // parenthesized expression, not a tuple
        if args.len() == 1 && !has_comma {
            self.insert_expression(TypedExpr::new(
                TypedExprKind::Paren(Paren::new(args[0])),
                syn.span(),
                self.type_of_expression(args[0]),
                self.empty_effect(),
            ))
        } else {
            let mut tuple_tys = Vec::new();

            for arg in args.iter().copied() {
                tuple_tys.push(self.type_of_expression(arg));
            }

            let ty = Ty::new_tuple(self.engine().intern_unsized(tuple_tys), self.engine());

            self.insert_expression(TypedExpr::new(
                TypedExprKind::Tuple(Tuple::new(args)),
                syn.span(),
                ty,
                self.empty_effect(),
            ))
        }
    }
}
