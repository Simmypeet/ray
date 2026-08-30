//! Type, effect-row, and signature resolution workflows.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_symbol::symbol_kind::SymbolKind;
use rayc_syntax::{
    effect_row::{EffectRow as EffectRowSyntax, EffectRowAnnotation},
    path::PathSegment,
    r#type::{Primitive as PrimitiveSyntax, Type as TypeSyntax},
};
use rayc_type::ty::{Mutability, Primitive, Ty, TyKind};

use crate::{is_poly_var_name, resolver::Resolver};

#[derive(Debug, Clone)]
pub struct ResolvedParameter {
    pub span: RelativeSpan,
    pub ty: Interned<Ty>,
}

/// The resolved semantic types and polymorphic environment of a function
/// signature.
#[derive(Debug, Clone)]
pub struct SignatureResolution {
    pub parameters: Vec<ResolvedParameter>,
    pub return_type: Interned<Ty>,
}

impl Resolver<'_> {
    fn infer_type_arguments(
        &mut self,
        identifier: &rayc_syntax::Identifier,
        expected: &[TyKind],
    ) -> Option<Interned<[Interned<Ty>]>> {
        if expected.is_empty() {
            return None;
        }

        let mut inferred = Vec::with_capacity(expected.len());
        for kind in expected {
            let Some(ty) = self.new_inference_type(*kind) else {
                self.report_type_inference_not_allowed(identifier, expected.len());
                return Some(self.intern_type_arguments(
                    expected.iter().map(|kind| self.new_error_type(*kind)).collect(),
                ));
            };
            inferred.push(ty);
        }
        Some(self.intern_type_arguments(inferred))
    }

    async fn resolve_explicit_type_arguments(
        &mut self,
        path: &PathSegment,
        expected: &[TyKind],
    ) -> Option<Interned<[Interned<Ty>]>> {
        let arguments = path.type_arguments()?;
        let actual = arguments.arguments().count();
        if actual != expected.len() {
            self.report_type_argument_arity_mismatch(arguments.span(), expected.len(), actual);
        }

        let mut resolved = Vec::new();
        for (index, argument) in arguments.arguments().enumerate() {
            let mut ty = Box::pin(self.resolve_type(&argument)).await;
            if let Some(expected) = expected.get(index) {
                let actual = self.type_kind(&ty).await;
                if actual != *expected {
                    self.report_type_kind_mismatch(argument.span(), *expected, actual);
                    ty = self.new_error_type(*expected);
                }
            }
            resolved.push(ty);
        }
        resolved.truncate(expected.len());
        resolved.extend(expected[resolved.len()..].iter().map(|kind| self.new_error_type(*kind)));
        Some(self.intern_type_arguments(resolved))
    }

    pub(crate) async fn resolve_type_arguments(
        &mut self,
        symbol_kind: SymbolKind,
        path: &PathSegment,
        identifier: &rayc_syntax::Identifier,
        expected: &[TyKind],
    ) -> Option<Interned<[Interned<Ty>]>> {
        if symbol_kind == SymbolKind::Def
            && let Some(arguments) = path.type_arguments()
        {
            self.report_explicit_type_arguments_not_allowed(arguments.span());
            return self.infer_type_arguments(identifier, expected);
        }

        if path.type_arguments().is_some() {
            self.resolve_explicit_type_arguments(path, expected).await
        } else {
            self.infer_type_arguments(identifier, expected)
        }
    }
}

impl Resolver<'_> {
    /// Resolves effect-row syntax relative to this resolver's site.
    #[must_use]
    pub async fn resolve_effect_row(&mut self, syntax: &EffectRowSyntax) -> Interned<Ty> {
        match syntax {
            EffectRowSyntax::PolyVar(identifier) => {
                self.new_checked_poly_var_type(identifier, TyKind::EffectRow).await
            }
            EffectRowSyntax::ConcreteEffectRow(effect_row) => {
                let mut labels = Vec::new();

                for path in effect_row.effects() {
                    let Ok(path_resolution) = self.resolve_effect_path(&path).await else {
                        continue;
                    };
                    let effect_symbol_id = path_resolution.symbol_id();
                    let arguments = path_resolution
                        .type_arguments()
                        .map_or_else(Vec::new, |arguments| arguments.cloned().collect());
                    labels.push(self.new_effect_label(effect_symbol_id, arguments));
                }

                let tail =
                    if let Some(variable) = effect_row.tail().and_then(|tail| tail.variable()) {
                        Some(self.new_checked_poly_var_type(&variable, TyKind::EffectRow).await)
                    } else {
                        None
                    };

                self.new_effect_row_type(labels, tail)
            }
        }
    }

    /// Resolves one type against this resolver's polymorphic environment.
    #[must_use]
    pub async fn resolve_type(&mut self, syntax: &TypeSyntax) -> Interned<Ty> {
        match syntax {
            TypeSyntax::Primitive(primitive) => {
                let primitive = match primitive {
                    PrimitiveSyntax::Int32(_) => Primitive::Int32,
                    PrimitiveSyntax::Bool(_) => Primitive::Bool,
                    PrimitiveSyntax::Float32(_) => Primitive::Float32,
                    PrimitiveSyntax::CInt(_) => Primitive::CInt,
                    PrimitiveSyntax::CStr(_) => Primitive::CStr,
                };
                self.new_primitive_type(primitive)
            }
            TypeSyntax::Pointer(ptr) => {
                let pointee = if let Some(pointed_type) = ptr.pointed_type() {
                    Box::pin(self.resolve_type(&pointed_type)).await
                } else {
                    self.new_error_type(TyKind::Star)
                };
                let mutability = if ptr.mut_keyword().is_some() {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                };
                self.new_pointer_type(pointee, mutability)
            }
            TypeSyntax::Tuple(tuple) => {
                let mut arguments = Vec::new();
                for element in tuple.elements() {
                    arguments.push(Box::pin(self.resolve_type(&element)).await);
                }
                self.new_tuple_type(arguments)
            }
            TypeSyntax::Lambda(lambda) => {
                let mut parameters = Vec::new();
                if let Some(parameter_list) = lambda.parameters() {
                    for parameter in parameter_list.parameters() {
                        parameters.push(Box::pin(self.resolve_type(&parameter)).await);
                    }
                }
                let return_type = if let Some(return_type) = lambda.return_type() {
                    if let Some(return_type) = return_type.r#type() {
                        Box::pin(self.resolve_type(&return_type)).await
                    } else {
                        self.new_error_type(TyKind::Star)
                    }
                } else {
                    self.new_unit_type()
                };
                let effect_row_syntax = lambda.effect_row();
                let effect_row = match effect_row_syntax
                    .as_ref()
                    .and_then(EffectRowAnnotation::effect_row)
                {
                    Some(effect_row) => self.resolve_effect_row(&effect_row).await,
                    None if effect_row_syntax.is_some() => self.new_error_type(TyKind::EffectRow),
                    None => self.new_effect_row_type(Vec::new(), None),
                };
                self.new_lambda_type(parameters, return_type, effect_row)
            }
            TypeSyntax::PolymorphicVariable(identifier) => {
                if !is_poly_var_name(&identifier.kind.0) {
                    return self.new_error_type(TyKind::Star);
                }
                self.new_poly_var_type(identifier, TyKind::Star)
            }
        }
    }
}
