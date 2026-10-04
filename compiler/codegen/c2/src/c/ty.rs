//! Rendering of C types, declarations, and function signatures.
//!
//! C spells a declaration "inside out": the declared identifier is nested in
//! pointer and function-pointer syntax around it, e.g. `int32_t *const *x`.
//! A [`Declarator`] is a stack-allocated linked list of those wrappers, built
//! while walking down a [`MonoType`] and rendered once the base type is
//! reached, so no intermediate strings are allocated.

use std::fmt::{self, Display};

use rayc_mono_ir::{
    function::MonoFunction,
    ty::{FunctionSignature, MonoType, PointerMutability, ReturnType},
};

use crate::c::name::{AggregateName, LocalName};

/// The part of a C declaration that surrounds the declared identifier.
#[derive(Clone, Copy)]
pub(crate) enum Declarator<'a> {
    /// No identifier, as in a type name used by a cast.
    Abstract,
    /// The declared identifier itself.
    Name(&'a dyn Display),
    /// `*inner`, or `* const inner` when the pointer object is `const`.
    Pointer { is_const: bool, inner: &'a Self },
    /// `(inner)`, required around a pointer to a function.
    Parenthesized(&'a Self),
}

impl fmt::Debug for Declarator<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Declarator({self})")
    }
}

impl Declarator<'_> {
    const fn is_abstract(&self) -> bool { matches!(self, Self::Abstract) }
}

impl Display for Declarator<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Abstract => Ok(()),
            Self::Name(name) => name.fmt(formatter),
            Self::Pointer { is_const: true, inner } => write!(formatter, "* const {inner}"),
            Self::Pointer { is_const: false, inner } => write!(formatter, "*{inner}"),
            Self::Parenthesized(inner) => write!(formatter, "({inner})"),
        }
    }
}

/// A C declaration of `declarator` with type `ty`, e.g. `int32_t ray_local_0`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Declaration<'a> {
    ty: &'a MonoType,
    declarator: Declarator<'a>,
    /// Whether the declared object itself is `const`.
    is_const: bool,
}

impl<'a> Declaration<'a> {
    pub(crate) const fn new(ty: &'a MonoType, name: &'a dyn Display) -> Self {
        Self { ty, declarator: Declarator::Name(name), is_const: false }
    }

    /// Writes `base declarator`, qualifying the base when the object is
    /// `const`.
    fn write_base(&self, formatter: &mut fmt::Formatter<'_>, base: &dyn Display) -> fmt::Result {
        if self.is_const {
            formatter.write_str("const ")?;
        }
        base.fmt(formatter)?;
        if self.declarator.is_abstract() {
            return Ok(());
        }
        write!(formatter, " {}", self.declarator)
    }

    /// Writes a pointer to the named `pointee` base type.
    fn write_pointer_to(&self, formatter: &mut fmt::Formatter<'_>, pointee: &str) -> fmt::Result {
        let declarator = Declarator::Pointer { is_const: self.is_const, inner: &self.declarator };
        write!(formatter, "{pointee} {declarator}")
    }
}

impl Display for Declaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty {
            MonoType::Bool => self.write_base(formatter, &"bool"),
            MonoType::Int8 => self.write_base(formatter, &"int8_t"),
            MonoType::Int16 => self.write_base(formatter, &"int16_t"),
            MonoType::Int32 => self.write_base(formatter, &"int32_t"),
            MonoType::Int64 => self.write_base(formatter, &"int64_t"),
            MonoType::Isize => self.write_base(formatter, &"intptr_t"),
            MonoType::Uint8 => self.write_base(formatter, &"uint8_t"),
            MonoType::Uint16 => self.write_base(formatter, &"uint16_t"),
            MonoType::Uint32 => self.write_base(formatter, &"uint32_t"),
            MonoType::Uint64 => self.write_base(formatter, &"uint64_t"),
            MonoType::Usize => self.write_base(formatter, &"uintptr_t"),
            MonoType::Float32 => self.write_base(formatter, &"float"),
            MonoType::CInt => self.write_base(formatter, &"int"),
            MonoType::Aggregate(aggregate) => {
                self.write_base(formatter, &AggregateName::of(aggregate).typedef())
            }

            MonoType::CStr => self.write_pointer_to(formatter, "const char"),
            MonoType::OpaquePointer(PointerMutability::Const) => {
                self.write_pointer_to(formatter, "const void")
            }
            MonoType::OpaquePointer(PointerMutability::Mut) => {
                self.write_pointer_to(formatter, "void")
            }

            // The pointer wraps the current declarator; the pointee's own
            // constness comes from the pointer's mutability.
            MonoType::Pointer(pointer) => Declaration {
                ty: pointer.pointee(),
                declarator: Declarator::Pointer {
                    is_const: self.is_const,
                    inner: &self.declarator,
                },
                is_const: pointer.mutability() == PointerMutability::Const,
            }
            .fmt(formatter),

            MonoType::FunctionPointer(signature) => {
                let pointer =
                    Declarator::Pointer { is_const: self.is_const, inner: &self.declarator };
                SignatureDeclaration {
                    signature,
                    declarator: Declarator::Parenthesized(&pointer),
                    parameters: ParameterNames::Unnamed,
                }
                .fmt(formatter)
            }
        }
    }
}

/// A C type name without an identifier, as used in casts and prototypes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeName<'a>(pub(crate) &'a MonoType);

impl Display for TypeName<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Declaration { ty: self.0, declarator: Declarator::Abstract, is_const: false }.fmt(formatter)
    }
}

/// Whether a signature's parameters are spelled with their local names.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ParameterNames<'a> {
    /// Parameters appear as bare type names, as in a function-pointer type.
    Unnamed,
    /// Parameters are named after the given function's parameter locals.
    Of(&'a MonoFunction),
}

/// A function declarator together with its return type, e.g.
/// `int32_t f(int32_t ray_local_0)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SignatureDeclaration<'a> {
    signature: &'a FunctionSignature,
    declarator: Declarator<'a>,
    parameters: ParameterNames<'a>,
}

impl<'a> SignatureDeclaration<'a> {
    /// Declares a function called `name` with unnamed parameters.
    pub(crate) const fn new(signature: &'a FunctionSignature, name: &'a dyn Display) -> Self {
        Self { signature, declarator: Declarator::Name(name), parameters: ParameterNames::Unnamed }
    }

    /// Declares the function-pointer object `declarator`.
    pub(crate) const fn function_pointer(
        signature: &'a FunctionSignature,
        declarator: &'a Declarator<'a>,
    ) -> Self {
        Self {
            signature,
            declarator: Declarator::Parenthesized(declarator),
            parameters: ParameterNames::Unnamed,
        }
    }

    /// Spells the parameters with the local names of `function`.
    pub(crate) const fn with_parameters_of(mut self, function: &'a MonoFunction) -> Self {
        self.parameters = ParameterNames::Of(function);
        self
    }

    fn write_parameters(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let types = self.signature.parameter_types();
        match self.parameters {
            ParameterNames::Unnamed => {
                write_separated(formatter, types.iter().map(|ty| TypeName(ty)))?;
            }
            ParameterNames::Of(function) => {
                assert_eq!(types.len(), function.parameters().len());
                let names = function.parameters().map(LocalName);
                let declarations =
                    types.iter().zip(names).map(|(ty, name)| OwnedNameDeclaration { ty, name });
                write_separated(formatter, declarations)?;
            }
        }

        match (self.signature.is_variadic(), types.is_empty()) {
            (true, true) => formatter.write_str("..."),
            (true, false) => formatter.write_str(", ..."),
            (false, true) => formatter.write_str("void"),
            (false, false) => Ok(()),
        }
    }
}

impl Display for SignatureDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.signature.return_type() {
            ReturnType::Void => formatter.write_str("void")?,
            ReturnType::Value(ty) => TypeName(ty).fmt(formatter)?,
        }
        write!(formatter, " {}(", self.declarator)?;
        self.write_parameters(formatter)?;
        formatter.write_str(")")
    }
}

/// A declaration whose name is held by value, so it can be produced by an
/// iterator adaptor.
struct OwnedNameDeclaration<'a> {
    ty: &'a MonoType,
    name: LocalName,
}

impl Display for OwnedNameDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Declaration::new(self.ty, &self.name).fmt(formatter)
    }
}

/// Writes `items` separated by `", "`.
pub(crate) fn write_separated(
    out: &mut impl fmt::Write,
    items: impl IntoIterator<Item = impl Display>,
) -> fmt::Result {
    for (index, item) in items.into_iter().enumerate() {
        if index != 0 {
            out.write_str(", ")?;
        }
        write!(out, "{item}")?;
    }
    Ok(())
}
