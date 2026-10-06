//! Rendering of C types, declarations, and function signatures.
//!
//! C spells a declaration "inside out": a base type followed by a declarator
//! that nests the identifier in pointer and function syntax, e.g.
//! `int32_t *const *x` or `int32_t (*f(void))(int32_t)`. A [`Declarator`] is a
//! stack-allocated linked list of those wrappers, built while walking down a
//! [`MonoType`] and rendered once the base type is reached, so no
//! intermediate strings are allocated.

use std::fmt::{self, Display};

use rayc_mono_ir::{
    function::MonoFunction,
    ty::{FunctionSignature, MonoType, PointerMutability, ReturnType},
};

use crate::c::name::{AggregateName, LocalName};

/// The part of a C declaration that surrounds the declared identifier.
#[derive(Clone, Copy)]
enum Declarator<'a> {
    /// No identifier, as in a type name used by a cast.
    Abstract,
    /// The declared identifier itself.
    Name(&'a dyn Display),
    /// `*inner`, or `*const inner` when the pointer object is `const`.
    Pointer { is_const: bool, inner: &'a Self },
    /// `inner(parameters)`: a function returning the base type.
    Function { inner: &'a Self, signature: &'a FunctionSignature, parameters: Parameters<'a> },
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
            Self::Pointer { is_const, inner } => {
                formatter.write_str("*")?;
                if *is_const {
                    formatter.write_str("const")?;
                    if !inner.is_abstract() {
                        formatter.write_str(" ")?;
                    }
                }
                inner.fmt(formatter)
            }
            // A postfix `(...)` binds tighter than a prefix `*`, so a pointer
            // being called through needs parentheses: `(*f)(int32_t)`.
            Self::Function { inner, signature, parameters } => {
                match inner {
                    Self::Pointer { .. } => write!(formatter, "({inner})"),
                    Self::Abstract | Self::Name(_) | Self::Function { .. } => inner.fmt(formatter),
                }?;
                write!(formatter, "({})", ParameterList { signature, parameters: *parameters })
            }
        }
    }
}

/// Writes `base declarator`, qualifying the base when the declared object is
/// `const`.
fn write_declaration(
    formatter: &mut fmt::Formatter<'_>,
    base: &dyn Display,
    is_const: bool,
    declarator: &Declarator<'_>,
) -> fmt::Result {
    if is_const {
        formatter.write_str("const ")?;
    }
    base.fmt(formatter)?;
    if declarator.is_abstract() {
        return Ok(());
    }
    write!(formatter, " {declarator}")
}

/// Writes a function returning the signature's return type, where
/// `declarator` is a [`Declarator::Function`].
fn write_function_declaration(
    formatter: &mut fmt::Formatter<'_>,
    signature: &FunctionSignature,
    declarator: Declarator<'_>,
) -> fmt::Result {
    match signature.return_type() {
        ReturnType::Void => write_declaration(formatter, &"void", false, &declarator),
        ReturnType::Value(ty) => Declaration { ty, declarator, is_const: false }.fmt(formatter),
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

    fn write_base(&self, formatter: &mut fmt::Formatter<'_>, base: &dyn Display) -> fmt::Result {
        write_declaration(formatter, base, self.is_const, &self.declarator)
    }

    /// Writes a pointer to the named `pointee` base type.
    fn write_pointer_to(&self, formatter: &mut fmt::Formatter<'_>, pointee: &str) -> fmt::Result {
        write_declaration(formatter, &pointee, false, &self.pointer_declarator())
    }

    /// The current declarator wrapped in a pointer, which carries the
    /// declared object's constness.
    const fn pointer_declarator(&self) -> Declarator<'_> {
        Declarator::Pointer { is_const: self.is_const, inner: &self.declarator }
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

            // The pointee is declared with the pointer wrapped around the
            // current declarator; its own constness comes from the pointer's
            // mutability.
            MonoType::Pointer(pointer) => Declaration {
                ty: pointer.pointee(),
                declarator: self.pointer_declarator(),
                is_const: pointer.mutability() == PointerMutability::Const,
            }
            .fmt(formatter),

            MonoType::FunctionPointer(signature) => {
                let pointer = self.pointer_declarator();
                let function = Declarator::Function {
                    inner: &pointer,
                    signature,
                    parameters: Parameters::Unnamed,
                };
                write_function_declaration(formatter, signature, function)
            }
        }
    }
}

/// A C type name without an identifier, as used in casts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeName<'a>(pub(crate) &'a MonoType);

impl Display for TypeName<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Declaration { ty: self.0, declarator: Declarator::Abstract, is_const: false }.fmt(formatter)
    }
}

/// Whether a signature's parameters are spelled with their local names.
#[derive(Debug, Clone, Copy)]
enum Parameters<'a> {
    /// Parameters appear as bare type names, as in a prototype.
    Unnamed,
    /// Parameters are named after the given function's parameter locals.
    NamedAfter(&'a MonoFunction),
}

/// The contents of a function declarator's parentheses.
struct ParameterList<'a> {
    signature: &'a FunctionSignature,
    parameters: Parameters<'a>,
}

impl Display for ParameterList<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let types = self.signature.parameter_types();
        match self.parameters {
            Parameters::Unnamed => {
                write_separated(formatter, types.iter().map(|ty| TypeName(ty)))?;
            }
            Parameters::NamedAfter(function) => {
                assert_eq!(types.len(), function.parameters().len());
                let names = function.parameters().map(LocalName);
                let declarations =
                    types.iter().zip(names).map(|(ty, name)| NamedParameter { ty, name });
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

/// A parameter declaration whose name is held by value, so it can be
/// produced by an iterator adaptor.
struct NamedParameter<'a> {
    ty: &'a MonoType,
    name: LocalName,
}

impl Display for NamedParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Declaration::new(self.ty, &self.name).fmt(formatter)
    }
}

/// A function, or a pointer to one, with the given signature, e.g.
/// `int32_t f(int32_t ray_local_0)` or `int32_t (*f)(int32_t)`.
#[derive(Clone, Copy)]
pub(crate) struct FunctionDeclaration<'a> {
    signature: &'a FunctionSignature,
    name: &'a dyn Display,
    parameters: Parameters<'a>,
    is_pointer: bool,
}

impl fmt::Debug for FunctionDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "FunctionDeclaration({self})")
    }
}

impl<'a> FunctionDeclaration<'a> {
    /// Declares a function called `name`.
    pub(crate) const fn new(signature: &'a FunctionSignature, name: &'a dyn Display) -> Self {
        Self { signature, name, parameters: Parameters::Unnamed, is_pointer: false }
    }

    /// Declares a function-pointer object called `name`.
    pub(crate) const fn pointer(signature: &'a FunctionSignature, name: &'a dyn Display) -> Self {
        Self { signature, name, parameters: Parameters::Unnamed, is_pointer: true }
    }

    /// Spells the parameters with the local names of `function`.
    pub(crate) const fn with_parameters_of(mut self, function: &'a MonoFunction) -> Self {
        self.parameters = Parameters::NamedAfter(function);
        self
    }
}

impl Display for FunctionDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = Declarator::Name(self.name);
        let pointer = Declarator::Pointer { is_const: false, inner: &name };
        let inner = if self.is_pointer { &pointer } else { &name };
        let function =
            Declarator::Function { inner, signature: self.signature, parameters: self.parameters };
        write_function_declaration(formatter, self.signature, function)
    }
}

/// Writes `items` separated by `", "`.
fn write_separated(
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
