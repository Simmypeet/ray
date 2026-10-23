use qbice::storage::intern::Interned;
use rayc_lexical::tree::{RelativeLocation, RelativeSpan};
use rayc_source_file::SourceElement;
use rayc_syntax::irrefutable_pattern::IrrefutablePattern as IrrefutablePatternSyntax;
use rayc_type::ty::Ty;
use rayc_typed_ast::name_binding::{NameBinding, NameBindingGroupID, NameBindingID, Source};

use crate::{
    diagnostic::{Diagnostic, DuplicateNameBinding},
    tast_builder::TAstBuilder,
};

/// A scope of the name environment and the name binding groups declared in
/// it.
#[derive(Debug)]
struct Scope {
    /// The syntax the scope covers. The names declared in the scope are
    /// visible up to its end.
    span: RelativeSpan,

    /// The name binding groups in scope at this level. The back is the most
    /// recently added group, and the front is the oldest.
    groups: Vec<NameBindingGroupID>,
}

#[derive(Debug)]
pub struct NameEnv {
    /// The scope stack. Entering a scope pushes a new [`Scope`] onto the
    /// stack, and exiting it pops it off.
    scopes: Vec<Scope>,
}

impl NameEnv {
    /// Creates the environment of a def whose body covers `span`, where its
    /// parameters, named in `first_name_binding_group`, are visible.
    #[must_use]
    pub fn new(first_name_binding_group: NameBindingGroupID, span: RelativeSpan) -> Self {
        Self { scopes: vec![Scope { span, groups: vec![first_name_binding_group] }] }
    }

    /// Enters a scope covering `span`, such as a block or the body of a
    /// nested function. The scope starts with `name_binding_group`, if any,
    /// such as the parameters of the nested function.
    pub(super) fn enter_scope(
        &mut self,
        span: RelativeSpan,
        name_binding_group: Option<NameBindingGroupID>,
    ) {
        self.scopes.push(Scope { span, groups: name_binding_group.into_iter().collect() });
    }

    pub(super) fn exit_scope(&mut self) {
        assert!(self.scopes.len() > 1, "the root name environment cannot be exited");
        self.scopes.pop();
    }
}

impl TAstBuilder {
    pub fn lookup_name_binding(&self, name: &str) -> Option<NameBindingID> {
        for outer in self.name_env.scopes.iter().rev() {
            for group_id in outer.groups.iter().rev() {
                if let Some(name_binding_id) =
                    self.function_map.lookup_name_binding(*group_id, name)
                {
                    return Some(name_binding_id);
                }
            }
        }

        None
    }

    pub fn insert_name_binding_to_group_from_pattern(
        &mut self,
        group_id: NameBindingGroupID,
        pat: &IrrefutablePatternSyntax,
        ty: &Interned<Ty>,
        source: Source,
    ) {
        match pat {
            IrrefutablePatternSyntax::Name(name) => {
                let Some(name_ident) = name.identifier() else {
                    return;
                };

                let name_binding = NameBinding::builder()
                    .ty(ty.clone())
                    .name(name_ident.kind.0.clone())
                    .source(source)
                    .mutable(name.mut_keyword().is_some())
                    .span(name.span())
                    .build();

                let name_binding_id = self.function_map.insert_name_binding(name_binding);

                if let Err(existing_id) =
                    self.function_map.insert_name_binding_to_group(group_id, name_binding_id)
                {
                    self.push_diagnostic(Diagnostic::DuplicateNameBinding(
                        DuplicateNameBinding::builder()
                            .existing_name_binding(
                                *self.function_map.get_name_binding(existing_id).span(),
                            )
                            .new_name_binding(name_ident.span())
                            .new_name(name_ident.kind.0)
                            .build(),
                    ));
                }
            }
        }
    }

    /// Adds a name binding group to the innermost scope, whose bindings are
    /// visible from `visible_from` to the end of the scope.
    pub fn push_new_name_binding_group(
        &mut self,
        visible_from: RelativeLocation,
    ) -> NameBindingGroupID {
        let scope = self.name_env.scopes.last_mut().expect("the root scope always exists");
        let visibility = RelativeSpan {
            start: visible_from,
            end: scope.span.end,
            source_id: scope.span.source_id,
        };

        let name_binding_group_id = self.function_map.new_name_binding_group(visibility);
        scope.groups.push(name_binding_group_id);

        name_binding_group_id
    }
}
