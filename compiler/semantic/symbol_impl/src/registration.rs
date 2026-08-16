use rayc_qbice::TrackedEngine;
use rayc_symbol::symbol_kind::SymbolKind;
use rayc_syntax::def::{Def, DefSignature};

use crate::table::{Infos, MemberBuilder, Table};

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

        let boddy = def.block();

        self.insert_symbol(
            member_builder,
            Infos::builder()
                .symbol_kind(SymbolKind::Def)
                .name(ident.kind.0.clone())
                .span(ident.span)
                .def_signature((def_param, def_return))
                .def_body(boddy)
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
        for member in module_content.defs() {
            self.register_def(member_builder, member.clone(), engine).await;
        }
    }
}
