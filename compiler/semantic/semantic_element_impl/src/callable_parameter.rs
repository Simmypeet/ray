use rayc_qbice::TrackedEngine;
use rayc_semantic_element::callable_parameter::{CallableParameter, Key};
use rayc_symbol::{
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_parameter_list_syntax,
};
use rayc_syntax::def::{ParameterEntry, ParameterType};

use crate::{
    build::{Build, Output},
    register_build,
};
impl Build for Key {
    type Diagnostic = rayc_resolution::Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let mut entries = Vec::new();

        // Only ordinary polymorphic definitions own generated binders.
        if engine.get_symbol_kind(symbol_id).await == SymbolKind::Def
            && let Some(parameters) = engine.get_parameter_list_syntax(symbol_id).await
        {
            for (index, parameter) in parameters
                .entries()
                .filter_map(|entry| match entry {
                    ParameterEntry::Parameter(parameter) => Some(parameter),
                    ParameterEntry::Ellipsis(_) => None,
                })
                .enumerate()
            {
                if let Some(ParameterType::CallableSugar(syntax)) = parameter.r#type() {
                    entries.push(CallableParameter::new(symbol_id, index, syntax));
                }
            }
        }
        Output::new(engine.intern_unsized(entries), engine)
    }
}
register_build!(Key);
