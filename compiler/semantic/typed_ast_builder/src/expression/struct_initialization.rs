use rayc_resolution::path::PathResolution;
use rayc_semantic_element::struct_body::get_struct_body;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::StructInitialization as StructInitializationSyntax;
use rayc_type::{subst::Substitutable, ty::Ty};
use rayc_typed_ast::typed_expr::{
    TypedExprID,
    struct_initialization::{FieldInitializer, StructInitialization},
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<StructInitializationSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: StructInitializationSyntax) -> TypedExprID {
        // Bind every initializer in source order before resolving the struct so
        // malformed initializations retain the effects of their value expressions.
        let mut fields = Vec::new();
        if let Some(initializations) = syn.fields() {
            for field in initializations.fields() {
                let name = field.name().map(|name| name.kind.0);
                let expression = if let Some(expression) = field.expression() {
                    Some(Box::pin(self.bind(expression)).await)
                } else {
                    None
                };
                fields.push((name, expression));
            }
        }

        let Some(path) = syn.path() else {
            return self
                .push_error_expression_with_expression_children(
                    syn.span(),
                    fields.into_iter().filter_map(|(_, expression)| expression),
                )
                .await;
        };
        let Ok(PathResolution::Struct(struct_)) = self.resolve_path(&path).await else {
            return self
                .push_error_expression_with_expression_children(
                    syn.span(),
                    fields.into_iter().filter_map(|(_, expression)| expression),
                )
                .await;
        };

        // Resolve field names while retaining the initializer order from the source.
        let body = self.engine().get_struct_body(struct_.symbol_id()).await;
        let subst = struct_.substitution(self.engine()).await;
        let mut initializers = Vec::with_capacity(fields.len());

        for (name, expression) in &fields {
            let (Some(name), Some(expression)) = (name, expression) else {
                return self
                    .push_error_expression_with_expression_children(
                        syn.span(),
                        fields.into_iter().filter_map(|(_, expression)| expression),
                    )
                    .await;
            };

            let Some((field_id, field)) = body.get_by_name(name) else {
                return self
                    .push_error_expression_with_expression_children(
                        syn.span(),
                        fields.into_iter().filter_map(|(_, expression)| expression),
                    )
                    .await;
            };

            let field_ty = field.ty().apply_subst_or_clone(&subst, self.engine());
            self.push_struct_field_initialization_constraint(&field_ty, *expression).await;
            initializers.push(FieldInitializer::new(field_id, *expression));
        }

        let ty = Ty::new_struct(struct_.symbol_id(), struct_.args().clone(), self.engine());
        self.insert_expression(
            StructInitialization::new(struct_.symbol_id(), initializers),
            syn.span(),
            ty,
        )
        .await
    }
}
