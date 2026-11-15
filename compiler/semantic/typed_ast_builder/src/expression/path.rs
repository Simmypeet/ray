//! Binds a path written as an expression.
//!
//! The parser reads `a.b.c` as a path wherever it is a leaf, as it cannot
//! tell a local followed by field accesses from a path naming a symbol. The
//! binder tells them apart as Rust does with its scopes: a first segment that
//! names a local makes the rest field accesses, and any other path is resolved
//! as a path, which only a variant can be the value of.

use rayc_resolution::path::PathResolution;
use rayc_source_file::SourceElement;
use rayc_syntax::path::{Path, PathContinuation};
use rayc_typed_ast::typed_expr::TypedExprID;

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, FieldTypeArguments, SymbolNotValue, UnboundName},
    tast_builder::TAstBuilder,
};

impl Bind<Path> for TAstBuilder {
    async fn bind(&mut self, syn: Path) -> TypedExprID {
        if let Some(bound) = self.bind_local_path(&syn).await {
            return bound;
        }

        // A variant is always named through its enum, so a bare name that is
        // not a local cannot be a value.
        if let Some(identifier) = syn.bare_identifier() {
            self.push_diagnostic(Diagnostic::UnboundName(
                UnboundName::builder()
                    .name(identifier.kind.0.clone())
                    .span(identifier.span)
                    .build(),
            ));
            return self.push_error_expression(syn.span()).await;
        }

        // The resolver reports a path that names nothing.
        let Ok(resolution) = self.resolve_path(&syn).await else {
            return self.push_error_expression(syn.span()).await;
        };

        match resolution {
            PathResolution::Variant(variant) => {
                self.build_variant(&variant, None, syn.span()).await
            }

            resolution => {
                if let Some(symbol_id) = resolution.global_id() {
                    self.push_diagnostic(Diagnostic::SymbolNotValue(
                        SymbolNotValue::builder().symbol_id(symbol_id).span(syn.span()).build(),
                    ));
                }
                self.push_error_expression(syn.span()).await
            }
        }
    }
}

impl TAstBuilder {
    /// Binds `path` as a local followed by field accesses, `p.x.y`, if its
    /// first segment names a local. Returns `None` otherwise, leaving the
    /// path to be resolved.
    ///
    /// Each segment after the first is recorded as a member access, as a
    /// field access after a `.` is, so that it can be completed.
    pub(super) async fn bind_local_path(&mut self, path: &Path) -> Option<TypedExprID> {
        let root = path.root_name()?;
        self.lookup_name_binding(&root.kind.0)?;

        let mut bound = self.bind(root).await;
        for continuation in path.rest() {
            bound = match self.build_path_field_access(bound, &continuation).await {
                Ok(bound) => bound,

                // keep the operand as the child of the error expression, so
                // that the errors in it are still reported
                Err(operand) => {
                    self.push_error_expression_with_children(path.span(), vec![operand.into()])
                        .await
                }
            };
        }

        Some(bound)
    }

    /// Binds the field access that the segment of `continuation` names on
    /// `bound`.
    ///
    /// If it fails, returns the dereferenced receiver, to keep as the child
    /// of the error expression standing for the access.
    async fn build_path_field_access(
        &mut self,
        bound: TypedExprID,
        continuation: &PathContinuation,
    ) -> Result<TypedExprID, TypedExprID> {
        let receiver = self.member_receiver_at(bound, continuation.span()).await;

        // a dangling `.`, whose field is still being typed
        let Some(segment) = continuation.segment() else { return Err(receiver) };
        let Some(name) = segment.identifier() else { return Err(receiver) };

        // the access is still bound, so that the rest of the body is checked
        if let Some(arguments) = segment.arguments() {
            self.push_diagnostic(Diagnostic::FieldTypeArguments(
                FieldTypeArguments::builder().span(arguments.span()).build(),
            ));
        }

        self.build_field_access(receiver, &name).await.ok_or(receiver)
    }
}
