use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::symbol_kind::SymbolKind;
use rayc_syntax::{
    def::{Def, DefSignature, ParameterEntry},
    module::ModuleMember,
};

use crate::{
    diagnostic::{Diagnostic, InvalidDefDeclaration, InvalidDefDeclarationKind},
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

        // very, very malformed node
        let Some(ident) = def_sig.as_ref().and_then(DefSignature::name) else {
            return;
        };

        let body = def.block();
        let is_extern =
            def_sig.as_ref().is_some_and(|signature| signature.extern_keyword().is_some());
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
                .def_signature((def_param, def_return))
                .def_body(body)
                .variadic(variadic)
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
                ModuleMember::Effect(_) => {}
            }
        }
    }
}
