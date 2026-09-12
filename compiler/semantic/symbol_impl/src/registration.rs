use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::symbol_kind::SymbolKind;
use rayc_syntax::{
    def::{Def, DefSignature, ParameterEntry},
    effect::{Effect, OperationSignature},
    extern_def::ExternDef,
    instance::{Instance, InstanceAssociatedType, InstanceMember},
    module::ModuleMember,
    r#trait::{Trait, TraitAssociatedType, TraitMember},
};

use crate::{
    diagnostic::{
        Diagnostic, InvalidDefDeclaration, InvalidDefDeclarationKind,
        InvalidEffectOperationDeclaration,
    },
    table::{Infos, MemberBuilder, Table},
};

impl Table {
    pub(crate) async fn register_def(
        &mut self,
        member_builder: &mut MemberBuilder,
        def: Def,
        engine: &TrackedEngine,
    ) {
        let Some(signature) = def.signature() else {
            return;
        };
        let Some(ident) = signature.name() else {
            return;
        };
        let parameters = signature.parameter_list();
        let ellipsis = parameters.as_ref().and_then(|parameters| {
            parameters.entries().find_map(|entry| match entry {
                ParameterEntry::Parameter(_) => None,
                ParameterEntry::Ellipsis(ellipsis) => Some(ellipsis),
            })
        });

        // Ordinary definitions cannot declare variadic parameters.
        if let Some(ellipsis) = ellipsis {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                InvalidDefDeclarationKind::NonExternVariadic,
                ellipsis.span(),
            )));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::Def)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(parameters)
                .given_parameter_list(signature.given_parameter_list())
                .where_clause(signature.where_clause())
                .return_type(signature.return_type())
                .effect_row(signature.effect_row())
                .def_body(def.block())
                .variadic(false)
                .build(),
            engine,
        )
        .await;
    }

    async fn register_extern_def(
        &mut self,
        member_builder: &mut MemberBuilder,
        def: ExternDef,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = def.name() else {
            return;
        };
        let parameters = def.parameter_list();

        // Extern definitions allow a single variadic marker at the end.
        let entries =
            parameters.as_ref().map(|parameters| parameters.entries().collect::<Vec<_>>());
        let variadic_index = entries.as_ref().and_then(|entries| {
            entries.iter().position(|entry| matches!(entry, ParameterEntry::Ellipsis(_)))
        });
        let variadic = variadic_index.is_some();
        let misplaced_variadic = variadic_index.is_some_and(|index| {
            entries.as_ref().is_some_and(|entries| index + 1 != entries.len())
        });

        if misplaced_variadic {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                InvalidDefDeclarationKind::VariadicNotLast,
                def.span(),
            )));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::ExternDef)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(parameters)
                .return_type(def.return_type())
                .variadic(variadic)
                .build(),
            engine,
        )
        .await;
    }

    async fn register_effect_operation(
        &mut self,
        member_builder: &mut MemberBuilder,
        operation: OperationSignature,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = operation.name() else {
            return;
        };
        let parameters = operation.parameter_list();
        let return_type = operation.return_type();
        let ellipsis = parameters.as_ref().and_then(|parameters| {
            parameters.entries().find_map(|entry| match entry {
                ParameterEntry::Parameter(_) => None,
                ParameterEntry::Ellipsis(ellipsis) => Some(ellipsis),
            })
        });
        if let Some(ellipsis) = ellipsis {
            self.push_diagnostic(Diagnostic::InvalidEffectOperationDeclaration(
                InvalidEffectOperationDeclaration::new(ellipsis.span()),
            ));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::EffectOperation)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(parameters)
                .return_type(return_type)
                .build(),
            engine,
        )
        .await;
    }

    async fn register_effect(
        &mut self,
        member_builder: &mut MemberBuilder,
        effect: Effect,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = effect.name() else {
            return;
        };
        let name = ident.kind.0.clone();
        let effect_id = self
            .insert_symbol(
                member_builder,
                Infos::builder()
                    .symbol_kind(SymbolKind::Effect)
                    .name(name.clone())
                    .span(ident.span)
                    .type_parameters(effect.type_parameters())
                    .given_parameter_list(effect.given_parameter_list())
                    .where_clause(effect.where_clause())
                    .build(),
                engine,
            )
            .await;

        let mut effect_members = member_builder.child(effect_id, name);
        if let Some(body) = effect.body() {
            for operation in body.operation_signatures() {
                self.register_effect_operation(&mut effect_members, operation.clone(), engine)
                    .await;
            }
        }

        self.insert_symbol_members(effect_id.id, effect_members, engine);
    }

    async fn register_trait_def(
        &mut self,
        member_builder: &mut MemberBuilder,
        signature: DefSignature,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = signature.name() else {
            return;
        };
        let parameters = signature.parameter_list();
        let ellipsis = parameters.as_ref().and_then(|parameters| {
            parameters.entries().find_map(|entry| match entry {
                ParameterEntry::Parameter(_) => None,
                ParameterEntry::Ellipsis(ellipsis) => Some(ellipsis),
            })
        });
        if let Some(ellipsis) = ellipsis {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                InvalidDefDeclarationKind::NonExternVariadic,
                ellipsis.span(),
            )));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::TraitDef)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(parameters)
                .given_parameter_list(signature.given_parameter_list())
                .where_clause(signature.where_clause())
                .return_type(signature.return_type())
                .effect_row(signature.effect_row())
                .build(),
            engine,
        )
        .await;
    }

    async fn register_instance_type(
        &mut self,
        member_builder: &mut MemberBuilder,
        ty: InstanceAssociatedType,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = ty.name() else {
            return;
        };

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::InstanceType)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .type_parameters(ty.type_parameters())
                .kind_ascription(ty.kind_ascription())
                .given_parameter_list(ty.given_parameter_list())
                .where_clause(ty.where_clause())
                .type_definition(ty.r#type())
                .build(),
            engine,
        )
        .await;
    }

    async fn register_trait_type(
        &mut self,
        member_builder: &mut MemberBuilder,
        ty: TraitAssociatedType,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = ty.name() else {
            return;
        };

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::TraitType)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .type_parameters(ty.type_parameters())
                .kind_ascription(ty.kind_ascription())
                .given_parameter_list(ty.given_parameter_list())
                .where_clause(ty.where_clause())
                .build(),
            engine,
        )
        .await;
    }

    async fn register_trait(
        &mut self,
        member_builder: &mut MemberBuilder,
        r#trait: Trait,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = r#trait.name() else {
            return;
        };
        let name = ident.kind.0.clone();
        let trait_id = self
            .insert_symbol(
                member_builder,
                Infos::builder()
                    .symbol_kind(SymbolKind::Trait)
                    .name(name.clone())
                    .span(ident.span)
                    .type_parameters(r#trait.type_parameters())
                    .given_parameter_list(r#trait.given_parameter_list())
                    .where_clause(r#trait.where_clause())
                    .build(),
                engine,
            )
            .await;

        let mut trait_members = member_builder.child(trait_id, name);
        if let Some(body) = r#trait.body() {
            for definition in body.definitions() {
                match definition {
                    TraitMember::Definition(signature) => {
                        self.register_trait_def(&mut trait_members, signature.clone(), engine)
                            .await;
                    }
                    TraitMember::AssociatedType(ty) => {
                        self.register_trait_type(&mut trait_members, ty.clone(), engine).await;
                    }
                }
            }
        }

        self.insert_symbol_members(trait_id.id, trait_members, engine);
    }

    async fn register_instance(
        &mut self,
        member_builder: &mut MemberBuilder,
        instance: Instance,
        engine: &TrackedEngine,
    ) {
        let Some(ident) = instance.name() else {
            return;
        };
        let name = ident.kind.0.clone();
        let instance_id = self
            .insert_symbol(
                member_builder,
                Infos::builder()
                    .symbol_kind(SymbolKind::Instance)
                    .name(name.clone())
                    .span(ident.span)
                    .type_parameters(instance.type_parameters())
                    .given_parameter_list(instance.given_parameter_list())
                    .where_clause(instance.where_clause())
                    .instance_trait(instance.trait_reference())
                    .build(),
                engine,
            )
            .await;

        let mut instance_members = member_builder.child(instance_id, name);
        if let Some(body) = instance.body() {
            for instance_def in body.definitions() {
                match instance_def {
                    InstanceMember::Definition(definition) => {
                        self.register_instance_def(
                            &mut instance_members,
                            definition.clone(),
                            engine,
                        )
                        .await;
                    }
                    InstanceMember::AssociatedType(ty) => {
                        self.register_instance_type(&mut instance_members, ty.clone(), engine)
                            .await;
                    }
                }
            }
        }

        self.insert_symbol_members(instance_id.id, instance_members, engine);
    }

    async fn register_instance_def(
        &mut self,
        member_builder: &mut MemberBuilder,
        def: Def,
        engine: &TrackedEngine,
    ) {
        let Some(signature) = def.signature() else {
            return;
        };
        let Some(ident) = signature.name() else {
            return;
        };
        let parameters = signature.parameter_list();
        let ellipsis = parameters.as_ref().and_then(|parameters| {
            parameters.entries().find_map(|entry| match entry {
                ParameterEntry::Parameter(_) => None,
                ParameterEntry::Ellipsis(ellipsis) => Some(ellipsis),
            })
        });
        if let Some(ellipsis) = ellipsis {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                InvalidDefDeclarationKind::NonExternVariadic,
                ellipsis.span(),
            )));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::InstanceDef)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(parameters)
                .given_parameter_list(signature.given_parameter_list())
                .where_clause(signature.where_clause())
                .return_type(signature.return_type())
                .effect_row(signature.effect_row())
                .def_body(def.block())
                .build(),
            engine,
        )
        .await;
    }

    pub(crate) async fn register_module_members(
        &mut self,
        member_builder: &mut MemberBuilder,
        module_content: &rayc_syntax::module::ModuleContent,
        engine: &TrackedEngine,
    ) {
        for member in module_content.members() {
            match member {
                ModuleMember::Def(def) => {
                    self.register_def(member_builder, def.clone(), engine).await;
                }
                ModuleMember::ExternDef(def) => {
                    self.register_extern_def(member_builder, def.clone(), engine).await;
                }
                ModuleMember::Effect(effect) => {
                    self.register_effect(member_builder, effect.clone(), engine).await;
                }
                ModuleMember::Trait(r#trait) => {
                    self.register_trait(member_builder, r#trait.clone(), engine).await;
                }
                ModuleMember::Instance(instance) => {
                    self.register_instance(member_builder, instance.clone(), engine).await;
                }
            }
        }
    }
}
