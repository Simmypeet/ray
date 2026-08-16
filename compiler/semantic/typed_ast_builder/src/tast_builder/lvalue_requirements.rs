use rayc_type::ty::{Mutability, Ty, TyApplicationView};
use rayc_typed_ast::typed_expr::{LvalueClassification, LvalueRoot, TypedExprID};

use crate::{
    diagnostic::{Diagnostic, ExpectedLvalue, ImmutableLvalue, LvalueOperation},
    tast_builder::TAstBuilder,
};

#[cfg(test)]
mod test;

#[derive(Debug)]
pub(super) struct LvalueRequirements {
    queued: Vec<LvalueRequirement>,
}

impl LvalueRequirements {
    #[must_use]
    pub(super) const fn new() -> Self { Self { queued: Vec::new() } }

    fn push(&mut self, requirement: LvalueRequirement) { self.queued.push(requirement); }

    fn take(&mut self) -> Vec<LvalueRequirement> { std::mem::take(&mut self.queued) }
}

#[derive(Debug, Clone, Copy)]
struct LvalueRequirement {
    expression: TypedExprID,
    mutable: bool,
    operation: LvalueOperation,
}

impl TAstBuilder {
    pub fn require_lvalue(
        &mut self,
        expression: TypedExprID,
        mutable: bool,
        operation: LvalueOperation,
    ) {
        self.lvalue_requirements.push(LvalueRequirement { expression, mutable, operation });
    }

    pub(super) fn validate_lvalue_requirements(&mut self) {
        for requirement in self.lvalue_requirements.take() {
            match self.building_function.classify_lvalue(requirement.expression) {
                LvalueClassification::Lvalue(root) => {
                    if requirement.mutable && self.lvalue_root_is_mutable(root) == Some(false) {
                        self.push_diagnostic(Diagnostic::ImmutableLvalue(
                            ImmutableLvalue::builder()
                                .operation(requirement.operation)
                                .span(self.span_of_expression(requirement.expression))
                                .build(),
                        ));
                    }
                }
                LvalueClassification::NotLvalue => {
                    self.push_diagnostic(Diagnostic::ExpectedLvalue(
                        ExpectedLvalue::builder()
                            .operation(requirement.operation)
                            .span(self.span_of_expression(requirement.expression))
                            .build(),
                    ));
                }
                LvalueClassification::Errored => {}
            }
        }
    }

    fn lvalue_root_is_mutable(&self, root: LvalueRoot) -> Option<bool> {
        match root {
            LvalueRoot::NameBinding(name_binding) => {
                Some(self.building_function.get_name_binding(name_binding).is_mutable())
            }
            LvalueRoot::Dereference(pointer) => {
                let ty = self.latest_type(&self.type_of_expression(pointer));

                match &*ty {
                    Ty::Application(application) => match application.view() {
                        TyApplicationView::Pointer(pointer) => {
                            Some(pointer.mutability() == Mutability::Mutable)
                        }
                        TyApplicationView::Primitive(_)
                        | TyApplicationView::Tuple(_)
                        | TyApplicationView::Error => None,
                    },
                    Ty::Inference(_) => None,
                }
            }
        }
    }
}
