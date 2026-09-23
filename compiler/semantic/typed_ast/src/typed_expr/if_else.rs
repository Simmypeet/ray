use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Arm {
    Expression(TypedExprID),
    Block(Vec<Statement>),
}

impl Arm {
    #[must_use]
    pub const fn expression(&self) -> Option<TypedExprID> {
        match self {
            Self::Expression(expression) => Some(*expression),
            Self::Block(_) => None,
        }
    }

    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        let expression = match self {
            Self::Expression(expression) => Some(*expression),
            Self::Block(_) => None,
        };
        let statements = match self {
            Self::Expression(_) => [].as_slice(),
            Self::Block(statements) => statements.as_slice(),
        };

        expression.into_iter().chain(statements.iter().flat_map(SubExprs::sub_exprs))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct ConditionalArm {
    condition: TypedExprID,
    arm: Arm,
}

impl ConditionalArm {
    #[must_use]
    pub const fn new(condition: TypedExprID, arm: Arm) -> Self { Self { condition, arm } }

    #[must_use]
    pub const fn condition(&self) -> TypedExprID { self.condition }

    #[must_use]
    pub const fn arm(&self) -> &Arm { &self.arm }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct IfElse {
    conditional_arms: Vec<ConditionalArm>,
    else_arm: Option<Arm>,
}

impl IfElse {
    #[must_use]
    pub const fn new(conditional_arms: Vec<ConditionalArm>, else_arm: Option<Arm>) -> Self {
        Self { conditional_arms, else_arm }
    }

    pub fn conditional_arms(&self) -> impl Iterator<Item = &ConditionalArm> {
        self.conditional_arms.iter()
    }

    #[must_use]
    pub const fn else_arm(&self) -> Option<&Arm> { self.else_arm.as_ref() }
}

impl MutSubstitutable for Arm {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        match self {
            Self::Expression(_) => {}
            Self::Block(statements) => {
                for statement in statements {
                    statement.apply_mut_subst(subst, engine);
                }
            }
        }
    }
}

impl MutSubstitutable for IfElse {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for conditional_arm in &mut self.conditional_arms {
            conditional_arm.arm.apply_mut_subst(subst, engine);
        }
        if let Some(else_arm) = &mut self.else_arm {
            else_arm.apply_mut_subst(subst, engine);
        }
    }
}

impl SubExprs for IfElse {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        self.conditional_arms
            .iter()
            .flat_map(|arm| std::iter::once(arm.condition()).chain(arm.arm().sub_exprs()))
            .chain(self.else_arm.iter().flat_map(Arm::sub_exprs))
    }
}
