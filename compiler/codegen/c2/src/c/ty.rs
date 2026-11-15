//! Rendering of C types, declarations, and function signatures.
//!
//! C spells a declaration "inside out": a base type followed by a declarator
//! that nests the identifier in pointer and function syntax, e.g.
//! `int32_t *const *x` or `int32_t (*f(void))(int32_t)`. A [`Declarator`] is a
//! stack-allocated linked list of those wrappers, built while walking down a
//! [`MonoType`] and rendered once the base type is reached, so no
//! intermediate strings are allocated.

use std::{
    borrow::Cow,
    fmt::{self, Display},
};

use rayc_mono_ir::{
    function::MonoFunction,
    ty::{
        AggregateType, EffectHandlerCallback, FunctionSignature, MonoType, PointerMutability,
        ReturnType,
    },
};

use crate::{
    aggregates::AggregateRegistry,
    c::name::{AggregateName, LocalName},
};

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
    Function {
        inner: &'a Self,
        signature: &'a FunctionSignature,
        parameters: Parameters<'a>,
        layouts: &'a AggregateRegistry,
    },
}

impl fmt::Debug for Declarator<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Declarator({self})")
    }
}

impl Declarator<'_> {
    const fn is_abstract(&self) -> bool { matches!(self, Self::Abstract) }

    /// Whether the declared object is a pointer to the base type, which C
    /// lets be incomplete.
    const fn is_pointer(&self) -> bool { matches!(self, Self::Pointer { .. }) }
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
            Self::Function { inner, signature, parameters, layouts } => {
                match inner {
                    Self::Pointer { .. } => write!(formatter, "({inner})"),
                    Self::Abstract | Self::Name(_) | Self::Function { .. } => inner.fmt(formatter),
                }?;
                let parameters = ParameterList { signature, parameters: *parameters, layouts };
                write!(formatter, "({parameters})")
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
/// `declarator` is a [`Declarator::Function`]. A function whose result is
/// zero-sized returns `void`.
fn write_function_declaration(
    formatter: &mut fmt::Formatter<'_>,
    signature: &FunctionSignature,
    declarator: Declarator<'_>,
    layouts: &AggregateRegistry,
) -> fmt::Result {
    match signature.return_type() {
        ReturnType::Value(ty) if !layouts.is_zero_sized(ty) => {
            Declaration { ty, declarator, is_const: false, layouts }.fmt(formatter)
        }
        ReturnType::Void | ReturnType::Value(_) => {
            write_declaration(formatter, &"void", false, &declarator)
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
    layouts: &'a AggregateRegistry,
}

impl<'a> Declaration<'a> {
    /// Declares `name`, whose type must not be zero-sized: no object of such
    /// a type is ever declared.
    pub(crate) const fn new(
        ty: &'a MonoType,
        name: &'a dyn Display,
        layouts: &'a AggregateRegistry,
    ) -> Self {
        Self { ty, declarator: Declarator::Name(name), is_const: false, layouts }
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
            // A zero-sized aggregate is never defined, so only a pointer to it
            // can be spelled.
            MonoType::Aggregate(aggregate) => {
                assert!(
                    self.declarator.is_pointer() || !self.layouts.is_zero_sized(self.ty),
                    "a zero-sized type is only spelled behind a pointer"
                );
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
                layouts: self.layouts,
            }
            .fmt(formatter),

            MonoType::FunctionPointer(signature) => {
                let pointer = self.pointer_declarator();
                let function = Declarator::Function {
                    inner: &pointer,
                    signature,
                    parameters: Parameters::Unnamed,
                    layouts: self.layouts,
                };
                write_function_declaration(formatter, signature, function, self.layouts)
            }
        }
    }
}

/// A `const pointee *name` declaration, given the pointee itself, so the
/// pointer type need not be interned. Without a name it is a type name.
#[derive(Clone, Copy)]
pub(crate) struct ConstPointerDeclaration<'a> {
    pointee: &'a MonoType,
    name: Option<&'a dyn Display>,
    layouts: &'a AggregateRegistry,
}

impl<'a> ConstPointerDeclaration<'a> {
    pub(crate) const fn new(
        pointee: &'a MonoType,
        name: &'a dyn Display,
        layouts: &'a AggregateRegistry,
    ) -> Self {
        Self { pointee, name: Some(name), layouts }
    }

    const fn type_name(pointee: &'a MonoType, layouts: &'a AggregateRegistry) -> Self {
        Self { pointee, name: None, layouts }
    }
}

impl fmt::Debug for ConstPointerDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ConstPointerDeclaration({self})")
    }
}

impl Display for ConstPointerDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.name.map_or(Declarator::Abstract, Declarator::Name);
        let declarator = Declarator::Pointer { is_const: false, inner: &inner };
        Declaration { ty: self.pointee, declarator, is_const: true, layouts: self.layouts }
            .fmt(formatter)
    }
}

/// A C type name without an identifier, as used in casts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeName<'a> {
    ty: &'a MonoType,
    layouts: &'a AggregateRegistry,
}

impl<'a> TypeName<'a> {
    pub(crate) const fn new(ty: &'a MonoType, layouts: &'a AggregateRegistry) -> Self {
        Self { ty, layouts }
    }
}

impl Display for TypeName<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let declarator = Declarator::Abstract;
        Declaration { ty: self.ty, declarator, is_const: false, layouts: self.layouts }
            .fmt(formatter)
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
///
/// A parameter of a zero-sized type is left out, since it passes nothing.
struct ParameterList<'a> {
    signature: &'a FunctionSignature,
    parameters: Parameters<'a>,
    layouts: &'a AggregateRegistry,
}

impl Display for ParameterList<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let signature = self.signature;
        let layouts = self.layouts;
        let written = match self.parameters {
            // The environment, the source parameters, then the handler
            // records, as `FunctionSignature` orders them in C. A handler
            // record is a pointer, which is never zero-sized.
            Parameters::Unnamed => {
                let environment = signature
                    .environment_slot()
                    .map(|slot| Cow::Owned(slot.ty()))
                    .filter(|ty| !layouts.is_zero_sized(ty))
                    .map(|ty| UnnamedParameter::Value(ty, layouts));
                let parameters = signature
                    .parameter_types()
                    .iter()
                    .filter(|ty| !layouts.is_zero_sized(ty))
                    .map(|ty| UnnamedParameter::Value(Cow::Borrowed(&**ty), layouts));
                let handler_records = signature.handler_record_slots().iter().map(|effect| {
                    UnnamedParameter::HandlerRecord(
                        MonoType::Aggregate(AggregateType::EffectHandlerCallback(
                            EffectHandlerCallback::new(effect.clone()),
                        )),
                        layouts,
                    )
                });
                write_separated(
                    formatter,
                    environment.into_iter().chain(parameters).chain(handler_records),
                )?
            }
            // The parameter locals are created in the same order, with the
            // type of each slot.
            Parameters::NamedAfter(function) => {
                assert_eq!(signature.abi_parameter_count(), function.abi_parameters().count());
                let declarations = function
                    .abi_parameters()
                    .map(|local| (local, function.get_local(local).ty()))
                    .filter(|(_, ty)| !layouts.is_zero_sized(ty))
                    .map(|(local, ty)| NamedParameter { ty, name: LocalName(local), layouts });
                write_separated(formatter, declarations)?
            }
        };

        match (signature.is_variadic(), written == 0) {
            (true, true) => formatter.write_str("..."),
            (true, false) => formatter.write_str(", ..."),
            (false, true) => formatter.write_str("void"),
            (false, false) => Ok(()),
        }
    }
}

/// A parameter of a prototype, which has no name.
enum UnnamedParameter<'a> {
    Value(Cow<'a, MonoType>, &'a AggregateRegistry),
    /// `const EffectHandler_E *`, given by the record type itself, since a
    /// prototype has no engine to intern the pointer type with.
    HandlerRecord(MonoType, &'a AggregateRegistry),
}

impl Display for UnnamedParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(ty, layouts) => TypeName::new(ty, layouts).fmt(formatter),
            Self::HandlerRecord(record, layouts) => {
                ConstPointerDeclaration::type_name(record, layouts).fmt(formatter)
            }
        }
    }
}

/// A parameter declaration whose name is held by value, so it can be
/// produced by an iterator adaptor.
struct NamedParameter<'a> {
    ty: &'a MonoType,
    name: LocalName,
    layouts: &'a AggregateRegistry,
}

impl Display for NamedParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Declaration::new(self.ty, &self.name, self.layouts).fmt(formatter)
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
    layouts: &'a AggregateRegistry,
}

impl fmt::Debug for FunctionDeclaration<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "FunctionDeclaration({self})")
    }
}

impl<'a> FunctionDeclaration<'a> {
    /// Declares a function called `name`.
    pub(crate) const fn new(
        signature: &'a FunctionSignature,
        name: &'a dyn Display,
        layouts: &'a AggregateRegistry,
    ) -> Self {
        Self { signature, name, parameters: Parameters::Unnamed, is_pointer: false, layouts }
    }

    /// Declares a function-pointer object called `name`.
    pub(crate) const fn pointer(
        signature: &'a FunctionSignature,
        name: &'a dyn Display,
        layouts: &'a AggregateRegistry,
    ) -> Self {
        Self { signature, name, parameters: Parameters::Unnamed, is_pointer: true, layouts }
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
        let function = Declarator::Function {
            inner,
            signature: self.signature,
            parameters: self.parameters,
            layouts: self.layouts,
        };
        write_function_declaration(formatter, self.signature, function, self.layouts)
    }
}

/// Writes `items` separated by `", "`, and returns how many there were.
fn write_separated(
    out: &mut impl fmt::Write,
    items: impl IntoIterator<Item = impl Display>,
) -> Result<usize, fmt::Error> {
    let mut count = 0;
    for item in items {
        if count != 0 {
            out.write_str(", ")?;
        }
        write!(out, "{item}")?;
        count += 1;
    }
    Ok(count)
}
