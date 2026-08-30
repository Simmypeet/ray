use qbice::storage::intern::Interned;
use rayc_source_file::SourceElement;
use rayc_syntax::irrefutable_pattern::IrrefutablePattern as IrrefutablePatternSyntax;
use rayc_type::ty::Ty;
use rayc_typed_ast::name_binding::{NameBinding, NameBindingGroupID, NameBindingID, Source};

use crate::{
    diagnostic::{Diagnostic, DuplicateNameBinding},
    tast_builder::TAstBuilder,
};

#[derive(Debug)]
pub struct NameEnv {
    /// The outer `Vec` represents the scope stack. You can imagine that once
    /// you enter a new scope, you push a new `Vec<NameBindingGroupID>` onto the
    /// stack, and when you exit a scope, you pop it off.
    ///
    /// The inner `Vec<NameBindingGroupID>` represents a sequence of name
    /// binding groups that are in scope at a particular level. The back of the
    /// inner `Vec` represents the most recently added name binding group, and
    /// the front represents the oldest.
    name_binding_gruop_stack: Vec<Vec<NameBindingGroupID>>,
}

impl NameEnv {
    #[must_use]
    pub fn new(first_name_binding_group: NameBindingGroupID) -> Self {
        Self { name_binding_gruop_stack: vec![vec![first_name_binding_group]] }
    }

    pub(super) fn enter_function(
        &mut self,
        parameter_name_binding_group: Option<NameBindingGroupID>,
    ) {
        self.name_binding_gruop_stack.push(parameter_name_binding_group.into_iter().collect());
    }

    pub(super) fn exit_function(&mut self) {
        assert!(
            self.name_binding_gruop_stack.len() > 1,
            "the root function name environment cannot be exited"
        );
        self.name_binding_gruop_stack.pop();
    }
}

impl TAstBuilder {
    pub fn lookup_name_binding(&self, name: &str) -> Option<NameBindingID> {
        for outer in self.name_env.name_binding_gruop_stack.iter().rev() {
            for group_id in outer.iter().rev() {
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

    pub fn push_new_name_binding_group(&mut self) -> NameBindingGroupID {
        let name_binding_group_id = self.function_map.new_name_binding_group();
        self.name_env.name_binding_gruop_stack.last_mut().unwrap().push(name_binding_group_id);

        name_binding_group_id
    }
}
