use rayc_hash::FxHashMap;
use rayc_resolution::path::PathResolution;
use rayc_semantic_element::struct_body::get_struct_body;
use rayc_source_file::SourceElement;
use rayc_syntax::{
    Identifier,
    expression::{StructFieldInitializations, StructInitialization as StructInitializationSyntax},
};
use rayc_type::subst::Substitutable;
use rayc_typed_ast::typed_expr::{
    TypedExprID,
    struct_initialization::{FieldInitializer, StructInitialization},
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, DuplicateStructFieldInitialization, MissingStructFieldInitialization,
        StructInitializationDiagnostic, UnknownStructField,
    },
    tast_builder::TAstBuilder,
};

type BoundField = (Option<Identifier>, Option<TypedExprID>);

impl Bind<StructInitializationSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: StructInitializationSyntax) -> TypedExprID {
        // Bind every initializer in source order before resolving the struct so
        // malformed initializations retain the effects of their value expressions.
        let fields = self.bind_struct_initializers(syn.fields()).await;

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
        let mut initialized_fields = FxHashMap::default();
        let mut has_error = false;

        for (name, expression) in &fields {
            let (Some(name), Some(expression)) = (name, expression) else {
                has_error = true;
                continue;
            };

            let Some((field_id, field)) = body.get_by_name(&name.kind) else {
                self.push_diagnostic(Diagnostic::StructInitialization(
                    StructInitializationDiagnostic::UnknownField(
                        UnknownStructField::builder()
                            .struct_id(struct_.symbol_id())
                            .name(name.kind.0.clone())
                            .span(name.span)
                            .build(),
                    ),
                ));
                has_error = true;
                continue;
            };

            if let Some(original_span) = initialized_fields.get(&field_id).copied() {
                self.push_diagnostic(Diagnostic::StructInitialization(
                    StructInitializationDiagnostic::DuplicateField(
                        DuplicateStructFieldInitialization::builder()
                            .name(name.kind.0.clone())
                            .original_span(original_span)
                            .duplicate_span(name.span)
                            .build(),
                    ),
                ));
                has_error = true;
            } else {
                initialized_fields.insert(field_id, name.span);
            }

            let field_ty = field.ty().apply_subst_or_clone(&subst, self.engine());
            self.push_struct_field_initialization_constraint(&field_ty, *expression).await;
            initializers.push(FieldInitializer::new(field_id, *expression));
        }

        // Diagnose absent fields in their declaration order for deterministic output.
        for (field_id, field) in body.iter() {
            if !initialized_fields.contains_key(&field_id) {
                self.push_diagnostic(Diagnostic::StructInitialization(
                    StructInitializationDiagnostic::MissingField(
                        MissingStructFieldInitialization::builder()
                            .struct_id(struct_.symbol_id())
                            .name(field.name().clone())
                            .initialization_span(syn.span())
                            .field_span(field.span())
                            .build(),
                    ),
                ));
                has_error = true;
            }
        }

        if has_error {
            return self
                .push_error_expression_with_expression_children(
                    syn.span(),
                    fields.into_iter().filter_map(|(_, expression)| expression),
                )
                .await;
        }

        self.insert_expression(
            StructInitialization::new(struct_.symbol_id(), initializers),
            syn.span(),
            struct_.into_struct_type(self.engine()),
        )
        .await
    }
}

impl TAstBuilder {
    async fn bind_struct_initializers(
        &mut self,
        initializations: Option<StructFieldInitializations>,
    ) -> Vec<BoundField> {
        let mut fields = Vec::new();
        if let Some(initializations) = initializations {
            for field in initializations.fields() {
                let name = field.name();
                let expression = if let Some(expression) = field.expression() {
                    Some(Box::pin(self.bind(expression)).await)
                } else {
                    None
                };
                fields.push((name, expression));
            }
        }
        fields
    }
}
