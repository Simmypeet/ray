use std::fmt;

use rayc_ir::{
    cfg::BlockID,
    ir_expr::ExpressionID,
    ir_function::FunctionID,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_variable::IRVariableID,
};
use rayc_semantic_element::parameter::ParameterID;

use crate::context::instantiation::{CLambdaTypeID, CTupleID, MonoFunctionSubstID};

#[derive(Debug, Clone, Copy)]
pub struct Identifier<'a>(IdentifierKind<'a>);

#[derive(Debug, Clone, Copy)]
enum IdentifierKind<'a> {
    Definition { name: &'a str, subst: MonoFunctionSubstID },
    LambdaDefinition { name: &'a str, subst: MonoFunctionSubstID, function: FunctionID },
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    Variable(IRVariableID),
    Expression(ExpressionID),
    Block(BlockID),
    PhiInput { predecessor: BlockID, successor: BlockID, phi: ExpressionID },
    TupleType(CTupleID),
    TupleStruct(CTupleID),
    TupleElement(usize),
    LambdaType(CLambdaTypeID),
    LambdaStruct(CLambdaTypeID),
    LambdaCallField,
    LambdaEnvField,
    LambdaEnvironment { name: &'a str, subst: MonoFunctionSubstID, function: FunctionID },
    LambdaEnvironmentValue(ExpressionID),
    LambdaRawEnvironment,
    LambdaTypedEnvironment,
    CaptureField(CaptureID),
    UnitField,
}

impl<'a> Identifier<'a> {
    #[must_use]
    pub(crate) const fn def(name: &'a str, subst: MonoFunctionSubstID) -> Self {
        Self(IdentifierKind::Definition { name, subst })
    }

    #[must_use]
    pub(crate) const fn lambda_def(
        name: &'a str,
        subst: MonoFunctionSubstID,
        function: FunctionID,
    ) -> Self {
        Self(IdentifierKind::LambdaDefinition { name, subst, function })
    }

    #[must_use]
    pub(crate) const fn lambda_environment(
        name: &'a str,
        subst: MonoFunctionSubstID,
        function: FunctionID,
    ) -> Self {
        Self(IdentifierKind::LambdaEnvironment { name, subst, function })
    }
}

impl Identifier<'static> {
    #[must_use]
    pub const fn param(id: ParameterID) -> Self { Self(IdentifierKind::Parameter(id)) }

    #[must_use]
    pub const fn lambda_param(id: LambdaParameterID) -> Self {
        Self(IdentifierKind::LambdaParameter(id))
    }

    #[must_use]
    pub const fn var(id: IRVariableID) -> Self { Self(IdentifierKind::Variable(id)) }

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
    pub const fn lambda_t(id: CLambdaTypeID) -> Self { Self(IdentifierKind::LambdaType(id)) }

    #[must_use]
    pub const fn lambda_struct(id: CLambdaTypeID) -> Self { Self(IdentifierKind::LambdaStruct(id)) }

    #[must_use]
    pub const fn lambda_call_field() -> Self { Self(IdentifierKind::LambdaCallField) }

    #[must_use]
    pub const fn lambda_env_field() -> Self { Self(IdentifierKind::LambdaEnvField) }

    #[must_use]
    pub const fn lambda_environment_value(expression: ExpressionID) -> Self {
        Self(IdentifierKind::LambdaEnvironmentValue(expression))
    }

    #[must_use]
    pub const fn lambda_raw_environment() -> Self { Self(IdentifierKind::LambdaRawEnvironment) }

    #[must_use]
    pub const fn lambda_typed_environment() -> Self { Self(IdentifierKind::LambdaTypedEnvironment) }

    #[must_use]
    pub const fn capture_field(id: CaptureID) -> Self { Self(IdentifierKind::CaptureField(id)) }

    #[must_use]
    pub const fn unit_field() -> Self { Self(IdentifierKind::UnitField) }
}

impl fmt::Display for Identifier<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            IdentifierKind::Definition { name, subst } => {
                write!(formatter, "ray_{name}_{}", subst.base62())
            }
            IdentifierKind::LambdaDefinition { name, subst, function } => {
                write!(formatter, "ray_{name}_{}_lambda_{:X}", subst.base62(), function.index())
            }
            IdentifierKind::Parameter(id) => write!(formatter, "ray_param_{:X}", id.index()),
            IdentifierKind::LambdaParameter(id) => {
                write!(formatter, "ray_lambda_param_{:X}", id.index())
            }
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
            IdentifierKind::LambdaType(id) => {
                write!(formatter, "RayLambda_{}_t", id.base62())
            }
            IdentifierKind::LambdaStruct(id) => {
                write!(formatter, "RayLambda_{}", id.base62())
            }
            IdentifierKind::LambdaCallField => formatter.write_str("call"),
            IdentifierKind::LambdaEnvField => formatter.write_str("env"),
            IdentifierKind::LambdaEnvironment { name, subst, function } => write!(
                formatter,
                "RayLambdaEnv_{name}_{}_lambda_{:X}",
                subst.base62(),
                function.index()
            ),
            IdentifierKind::LambdaEnvironmentValue(expression) => {
                write!(formatter, "ray_lambda_env_{:X}", expression.index())
            }
            IdentifierKind::LambdaRawEnvironment => formatter.write_str("ray_raw_env"),
            IdentifierKind::LambdaTypedEnvironment => formatter.write_str("ray_env"),
            IdentifierKind::CaptureField(id) => {
                write!(formatter, "ray_capture_{:X}", id.index())
            }
            IdentifierKind::UnitField => formatter.write_str("_unit"),
        }
    }
}
