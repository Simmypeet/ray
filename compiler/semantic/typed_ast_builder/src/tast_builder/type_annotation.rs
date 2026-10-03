use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::Subst,
    ty::{Ty, TyKind, inference::Inference},
};
use rayc_typed_ast::{
    name_binding::{NameBinding, Source},
    typed_expr::{TypedExpr, TypedExprKind},
    typed_function::TypedFunctionMap,
};

use crate::diagnostic::{Diagnostic, TypeAnnotationRequired, TypeAnnotationSubject};

/// Reports every type that the constraints left undetermined, once the
/// solved substitution has been applied to `function_map`.
///
/// Each undetermined type is reported once, at the first site that mentions
/// it. Instantiation sites are visited first, since they name the type
/// parameter that is left undetermined, e.g. `a` in `def make[a]() -> int32`.
/// Name bindings follow, such as a lambda parameter that no call determines,
/// and the remaining expressions are visited last.
pub(super) async fn type_annotation_required_diagnostics(
    function_map: &TypedFunctionMap,
    engine: &TrackedEngine,
) -> Vec<Diagnostic> {
    let mut reported = FxHashSet::default();
    let mut diagnostics = Vec::new();

    let expressions = || {
        function_map.functions().flat_map(|(function_id, function)| {
            function.expressions().map(move |(expr_id, expr)| ((function_id, expr_id), expr))
        })
    };

    // Report the type parameters left undetermined at each instantiation site,
    // in declaration order.
    for (_, expression) in expressions() {
        let Some(instantiation) = instantiation_of(expression, engine).await else {
            continue;
        };

        for (poly_var_id, ty) in instantiation.poly_var_mappings() {
            if mark_undetermined_inferences(ty, &mut reported) {
                diagnostics.push(Diagnostic::TypeAnnotationRequired(
                    TypeAnnotationRequired::builder()
                        .subject(TypeAnnotationSubject::TypeParameter(poly_var_id))
                        .ty(ty.clone())
                        .span(expression.span())
                        .build(),
                ));
            }
        }
    }

    // Arena iteration is unordered, so visit the name bindings in creation
    // order to report the earliest binding deterministically.
    for (_, name_binding) in function_map.name_bindings() {
        if mark_undetermined_inferences(name_binding.ty(), &mut reported) {
            diagnostics.push(Diagnostic::TypeAnnotationRequired(
                TypeAnnotationRequired::builder()
                    .subject(TypeAnnotationSubject::of_name_binding(name_binding))
                    .ty(name_binding.ty().clone())
                    .span(*name_binding.span())
                    .build(),
            ));
        }
    }

    // The remaining undetermined types flow through no instantiation nor name
    // binding.
    for (_, expression) in expressions() {
        if mark_undetermined_inferences(expression.ty(), &mut reported) {
            diagnostics.push(Diagnostic::TypeAnnotationRequired(
                TypeAnnotationRequired::builder()
                    .subject(TypeAnnotationSubject::Expression)
                    .ty(expression.ty().clone())
                    .span(expression.span())
                    .build(),
            ));
        }
    }

    diagnostics
}

/// The substitution instantiating the type parameters of the symbol that
/// `expression` instantiates, if it is an instantiation site.
async fn instantiation_of(expression: &TypedExpr, engine: &TrackedEngine) -> Option<Subst> {
    match expression.kind() {
        TypedExprKind::Call(call) => Some(call.instantiation().clone()),

        // A struct initialization carries its type arguments in its type.
        TypedExprKind::StructInitialization(_) => {
            let struct_view = expression
                .ty()
                .as_struct_view()
                .expect("a struct initialization should have a struct type");

            Some(struct_view.create_subst(engine).await)
        }

        TypedExprKind::Identifier(_)
        | TypedExprKind::Literal(_)
        | TypedExprKind::TupleIndex(_)
        | TypedExprKind::FieldAccess(_)
        | TypedExprKind::Tuple(_)
        | TypedExprKind::Closure(_)
        | TypedExprKind::Binary(_)
        | TypedExprKind::Unary(_)
        | TypedExprKind::Cast(_)
        | TypedExprKind::IfElse(_)
        | TypedExprKind::While(_)
        | TypedExprKind::RefOf(_)
        | TypedExprKind::Deref(_)
        | TypedExprKind::Move(_)
        | TypedExprKind::Paren(_)
        | TypedExprKind::RunWith(_)
        | TypedExprKind::StatementBlock(_)
        | TypedExprKind::RefToPointer(_)
        | TypedExprKind::Errored(_) => None,
    }
}

/// Marks the undetermined types mentioned by `ty` as reported. Returns
/// whether any of them was not reported before.
///
/// Only inferences of kind [`TyKind::Star`] are considered: effect rows are
/// defaulted, instances are reported by instance resolution, and lifetimes are
/// erased.
fn mark_undetermined_inferences(ty: &Interned<Ty>, reported: &mut FxHashSet<Inference>) -> bool {
    let mut introduced = false;

    for inference in ty.recursive_iter().filter_map(Ty::as_inference) {
        if inference.kind() == TyKind::Star {
            introduced |= reported.insert(*inference);
        }
    }

    introduced
}

impl TypeAnnotationSubject {
    /// Names `name_binding` as the site that needs an annotation.
    fn of_name_binding(name_binding: &NameBinding) -> Self {
        let name = name_binding.name().clone();

        match name_binding.source() {
            Source::LambdaParameter(_) => Self::LambdaParameter(name),
            Source::Variable(_) | Source::Parameter(_) | Source::OperationHandlerParameter(_) => {
                Self::NameBinding(name)
            }
        }
    }
}
