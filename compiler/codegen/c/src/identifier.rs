use std::fmt;

use rayc_ir::{cfg::BlockID, expression::ExpressionID, variable::VariableID};
use rayc_semantic_element::parameter::ParameterID;

use crate::context::instantiation::CTupleID;

#[derive(Debug, Clone, Copy)]
pub struct Identifier<'a>(IdentifierKind<'a>);

#[derive(Debug, Clone, Copy)]
enum IdentifierKind<'a> {
    Definition(&'a str),
    Parameter(ParameterID),
    Variable(VariableID),
    Expression(ExpressionID),
    Block(BlockID),
    PhiInput { predecessor: BlockID, successor: BlockID, phi: ExpressionID },
    TupleType(CTupleID),
    TupleStruct(CTupleID),
    TupleElement(usize),
    UnitField,
}

impl<'a> Identifier<'a> {
    #[must_use]
    pub const fn def(name: &'a str) -> Self { Self(IdentifierKind::Definition(name)) }
}

impl Identifier<'static> {
    #[must_use]
    pub const fn param(id: ParameterID) -> Self { Self(IdentifierKind::Parameter(id)) }

    #[must_use]
    pub const fn var(id: VariableID) -> Self { Self(IdentifierKind::Variable(id)) }

    #[must_use]
    pub const fn expr(id: ExpressionID) -> Self { Self(IdentifierKind::Expression(id)) }

    #[must_use]
    pub const fn block(id: BlockID) -> Self { Self(IdentifierKind::Block(id)) }

    #[must_use]
    pub const fn phi_input(predecessor: BlockID, successor: BlockID, phi: ExpressionID) -> Self {
        Self(IdentifierKind::PhiInput { predecessor, successor, phi })
    }

    #[must_use]
    pub const fn tuple_t(id: CTupleID) -> Self { Self(IdentifierKind::TupleType(id)) }

    #[must_use]
    pub const fn tuple_struct(id: CTupleID) -> Self { Self(IdentifierKind::TupleStruct(id)) }

    #[must_use]
    pub const fn tuple_elem(index: usize) -> Self { Self(IdentifierKind::TupleElement(index)) }

    #[must_use]
    pub const fn unit_field() -> Self { Self(IdentifierKind::UnitField) }
}

impl fmt::Display for Identifier<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            IdentifierKind::Definition(name) => write!(formatter, "ray_{name}"),
            IdentifierKind::Parameter(id) => write!(formatter, "ray_param_{:X}", id.index()),
            IdentifierKind::Variable(id) => write!(formatter, "ray_var_{:X}", id.index()),
            IdentifierKind::Expression(id) => write!(formatter, "ray_expr_{:X}", id.index()),
            IdentifierKind::Block(id) => write!(formatter, "ray_block_{:X}", id.index()),
            IdentifierKind::PhiInput { predecessor, successor, phi } => write!(
                formatter,
                "ray_phi_in_{:X}_{:X}_{:X}",
                predecessor.index(),
                successor.index(),
                phi.index()
            ),
            IdentifierKind::TupleType(id) => write!(formatter, "RayTuple_{}_t", id.base62()),
            IdentifierKind::TupleStruct(id) => write!(formatter, "RayTuple_{}", id.base62()),
            IdentifierKind::TupleElement(index) => write!(formatter, "elem{index:X}"),
            IdentifierKind::UnitField => formatter.write_str("_unit"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn local_identifiers_use_the_c_backend_naming_convention() {
        assert_eq!(Identifier::def("produce").to_string(), "ray_produce");
        assert_eq!(Identifier::param(ParameterID::new(10)).to_string(), "ray_param_A");
        assert_eq!(Identifier::var(VariableID::new(11)).to_string(), "ray_var_B");
        assert_eq!(Identifier::expr(ExpressionID::new(12)).to_string(), "ray_expr_C");
        assert_eq!(Identifier::block(BlockID::new(13)).to_string(), "ray_block_D");
        assert_eq!(
            Identifier::phi_input(BlockID::new(1), BlockID::new(2), ExpressionID::new(10))
                .to_string(),
            "ray_phi_in_1_2_A"
        );
        assert_eq!(Identifier::tuple_elem(10).to_string(), "elemA");
        assert_eq!(Identifier::unit_field().to_string(), "_unit");
    }
}
