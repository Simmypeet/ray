use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::symbol_kind::SymbolKind;
use rayc_syntax::{
    def::{Def, DefSignature, ParameterEntry},
    effect::{Effect, OperationSignature},
    module::ModuleMember,
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
        let def_sig = def.signature();

        let def_param = def_sig.as_ref().and_then(DefSignature::parameter_list);
        let def_return = def_sig.as_ref().and_then(DefSignature::return_type);
        let def_effect_row = def_sig.as_ref().and_then(DefSignature::effect_row);

        // very, very malformed node
        let Some(ident) = def_sig.as_ref().and_then(DefSignature::name) else {
            return;
        };

        let body = def.block();
        let is_extern =
            def_sig.as_ref().is_some_and(|signature| signature.extern_keyword().is_some());
        if is_extern && let Some(effect_row) = def_effect_row.as_ref() {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                InvalidDefDeclarationKind::ExternHasEffectRow,
                effect_row.span(),
            )));
        }
        let registered_effect_row = if is_extern { None } else { def_effect_row };
        let entries = def_param.as_ref().map(|parameters| parameters.entries().collect::<Vec<_>>());
        let variadic_index = entries.as_ref().and_then(|entries| {
            entries.iter().position(|entry| matches!(entry, ParameterEntry::Ellipsis(_)))
        });
        let variadic = variadic_index.is_some();
        let misplaced_variadic = variadic_index.is_some_and(|index| {
            entries.as_ref().is_some_and(|entries| index + 1 != entries.len())
        });

        let invalid_kind = match (is_extern, body.is_some(), variadic, misplaced_variadic) {
            (true, true, _, _) => Some(InvalidDefDeclarationKind::ExternHasBody),
            (false, false, _, _) => Some(InvalidDefDeclarationKind::DefMissingBody),
            (_, _, _, true) => Some(InvalidDefDeclarationKind::VariadicNotLast),
            (false, true, true, false) => Some(InvalidDefDeclarationKind::NonExternVariadic),
            (true, false, _, false) | (false, true, false, false) => None,
        };
        if let Some(kind) = invalid_kind {
            self.push_diagnostic(Diagnostic::InvalidDefDeclaration(InvalidDefDeclaration::new(
                kind,
                def.span(),
            )));
        }

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(if is_extern { SymbolKind::ExternDef } else { SymbolKind::Def })
                .name(ident.kind.0.clone())
                .span(ident.span)
                .parameter_list(def_param)
                .return_type(def_return)
                .effect_row(registered_effect_row)
                .def_body(body)
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
                    .effect_type_parameters(effect.type_parameters())
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
                ModuleMember::Effect(effect) => {
                    self.register_effect(member_builder, effect.clone(), engine).await;
                }
                ModuleMember::Trait(_) => {}
            }
        }
    }
}
