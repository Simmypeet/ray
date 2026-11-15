//! Rendering of C types, declarations, and function signatures.
//!
//! C spells a declaration "inside out": a base type followed by a declarator
//! that nests the identifier in pointer and function syntax, e.g.
//! `int32_t *const *x` or `int32_t (*f(void))(int32_t)`. A [`Declarator`] is a
//! stack-allocated linked list of those wrappers, built while walking down a
//! [`MonoType`] and rendered once the base type is reached, so no
//! intermediate strings are allocated.
//!
//! Spelling a type asks the engine whether it is zero-sized, since a value
//! that holds nothing is never declared, passed or returned; so the types
//! here implement [`WriteC`] rather than [`Display`]. A type nests other types,
//! so the calls that can recurse are boxed. The recursion goes through the
//! inherent `write_to` methods, never through [`WriteC`]: an `async fn` of a
//! trait implementation cannot await itself, even boxed.

use std::fmt::Display;

use rayc_mono_ir::{
    function::MonoFunction,
    ty::{
        AggregateType, EffectHandlerCallback, FunctionSignature, MonoType, PointerMutability,
        ReturnType,
    },
};

use crate::c::{
    name::{AggregateName, LocalName},
    writer::{CWriter, WriteC},
};

/// A name a declaration declares, which a future of [`WriteC`] may hold
/// across an `.await`, so it must be shareable between threads.
type DeclaredName<'a> = &'a (dyn Display + Sync);

/// The part of a C declaration that surrounds the declared identifier.
#[derive(Clone, Copy)]
enum Declarator<'a> {
    /// No identifier, as in a type name used by a cast.
    Abstract,
    /// The declared identifier itself.
    Name(DeclaredName<'a>),
    /// `*inner`, or `*const inner` when the pointer object is `const`.
    Pointer { is_const: bool, inner: &'a Self },
    /// `inner(parameters)`: a function returning the base type.
    Function { inner: &'a Self, signature: &'a FunctionSignature, parameters: Parameters<'a> },
}

impl Declarator<'_> {
    const fn is_abstract(&self) -> bool { matches!(self, Self::Abstract) }

    /// Whether the declared object is a pointer to the base type, which C
    /// lets be incomplete.
    const fn is_pointer(&self) -> bool { matches!(self, Self::Pointer { .. }) }

    async fn write_c(&self, writer: &mut CWriter<'_>) {
        match self {
            Self::Abstract => {}
            Self::Name(name) => writer.push(name),
            Self::Pointer { is_const, inner } => {
                writer.push("*");
                if *is_const {
                    writer.push("const");
                    if !inner.is_abstract() {
                        writer.push(" ");
                    }
                }
                Box::pin(inner.write_c(writer)).await;
            }
            // a postfix `(...)` binds tighter than a prefix `*`, so a pointer
            // being called through needs parentheses: `(*f)(int32_t)`
            Self::Function { inner, signature, parameters } => {
                match inner {
                    Self::Pointer { .. } => {
                        writer.push("(");
                        Box::pin(inner.write_c(writer)).await;
                        writer.push(")");
                    }
                    Self::Abstract | Self::Name(_) | Self::Function { .. } => {
                        Box::pin(inner.write_c(writer)).await;
                    }
                }
                writer.push("(");
                let parameters = ParameterList { signature, parameters: *parameters };
                Box::pin(parameters.write_c(writer)).await;
                writer.push(")");
            }
        }
    }
}

/// Writes `base declarator`, qualifying the base when the declared object is
/// `const`.
async fn write_declaration(
    writer: &mut CWriter<'_>,
    base: impl Display,
    is_const: bool,
    declarator: &Declarator<'_>,
) {
    if is_const {
        writer.push("const ");
    }
    writer.push(base);
    if declarator.is_abstract() {
        return;
    }
    writer.push(" ");
    declarator.write_c(writer).await;
}

/// Writes a function returning the signature's return type, where
/// `declarator` is a [`Declarator::Function`]. A function whose result is
/// zero-sized returns `void`.
async fn write_function_declaration(
    writer: &mut CWriter<'_>,
    signature: &FunctionSignature,
    declarator: Declarator<'_>,
) {
    match signature.return_type() {
        ReturnType::Value(ty) if !writer.is_zero_sized(ty).await => {
            let declaration = Declaration { ty, declarator, is_const: false };
            Box::pin(declaration.write_to(writer)).await;
        }
        ReturnType::Void | ReturnType::Value(_) => {
            write_declaration(writer, "void", false, &declarator).await;
        }
    }
}

/// A C declaration of `declarator` with type `ty`, e.g. `int32_t ray_local_0`.
#[derive(Clone, Copy)]
pub(crate) struct Declaration<'a> {
    ty: &'a MonoType,
    declarator: Declarator<'a>,
    /// Whether the declared object itself is `const`.
    is_const: bool,
}

impl<'a> Declaration<'a> {
    /// Declares `name`, whose type must not be zero-sized: no object of such
    /// a type is ever declared.
    pub(crate) const fn new(ty: &'a MonoType, name: DeclaredName<'a>) -> Self {
        Self { ty, declarator: Declarator::Name(name), is_const: false }
    }

    async fn write_base(&self, writer: &mut CWriter<'_>, base: impl Display) {
        write_declaration(writer, base, self.is_const, &self.declarator).await;
    }

    /// Writes a pointer to the named `pointee` base type.
    async fn write_pointer_to(&self, writer: &mut CWriter<'_>, pointee: &str) {
        write_declaration(writer, pointee, false, &self.pointer_declarator()).await;
    }

    /// The current declarator wrapped in a pointer, which carries the
    /// declared object's constness.
    const fn pointer_declarator(&self) -> Declarator<'_> {
        Declarator::Pointer { is_const: self.is_const, inner: &self.declarator }
    }
}

impl WriteC for Declaration<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) { self.write_to(writer).await }
}

impl Declaration<'_> {
    async fn write_to(&self, writer: &mut CWriter<'_>) {
        match self.ty {
            MonoType::Bool => self.write_base(writer, "bool").await,
            MonoType::Int8 => self.write_base(writer, "int8_t").await,
            MonoType::Int16 => self.write_base(writer, "int16_t").await,
            MonoType::Int32 => self.write_base(writer, "int32_t").await,
            MonoType::Int64 => self.write_base(writer, "int64_t").await,
            MonoType::Isize => self.write_base(writer, "intptr_t").await,
            MonoType::Uint8 => self.write_base(writer, "uint8_t").await,
            MonoType::Uint16 => self.write_base(writer, "uint16_t").await,
            MonoType::Uint32 => self.write_base(writer, "uint32_t").await,
            MonoType::Uint64 => self.write_base(writer, "uint64_t").await,
            MonoType::Usize => self.write_base(writer, "uintptr_t").await,
            MonoType::Float32 => self.write_base(writer, "float").await,
            MonoType::CInt => self.write_base(writer, "int").await,
            // the only place an aggregate is spelled, so the only place it is
            // recorded; a zero-sized aggregate is never defined, so only a
            // pointer to it can be spelled
            MonoType::Aggregate(aggregate) => {
                if !self.declarator.is_pointer() {
                    assert!(
                        !writer.is_zero_sized(self.ty).await,
                        "a zero-sized type is only spelled behind a pointer"
                    );
                }
                writer.record_aggregate(aggregate);
                self.write_base(writer, AggregateName::of(aggregate).typedef()).await;
            }

            MonoType::CStr => self.write_pointer_to(writer, "const char").await,
            MonoType::OpaquePointer(PointerMutability::Const) => {
                self.write_pointer_to(writer, "const void").await;
            }
            MonoType::OpaquePointer(PointerMutability::Mut) => {
                self.write_pointer_to(writer, "void").await;
            }

            // the pointee is declared with the pointer wrapped around the
            // current declarator; its own constness comes from the pointer's
            // mutability
            MonoType::Pointer(pointer) => {
                let pointee_declaration = Declaration {
                    ty: pointer.pointee(),
                    declarator: self.pointer_declarator(),
                    is_const: pointer.mutability() == PointerMutability::Const,
                };
                Box::pin(pointee_declaration.write_to(writer)).await;
            }

            MonoType::FunctionPointer(signature) => {
                let pointer = self.pointer_declarator();
                let function = Declarator::Function {
                    inner: &pointer,
                    signature,
                    parameters: Parameters::Unnamed,
                };
                write_function_declaration(writer, signature, function).await;
            }
        }
    }
}

/// A `const pointee *` type name, given the pointee itself, so the pointer
/// type need not be interned.
#[derive(Clone, Copy)]
struct ConstPointerTypeName<'a> {
    pointee: &'a MonoType,
}

impl ConstPointerTypeName<'_> {
    async fn write_to(&self, writer: &mut CWriter<'_>) {
        let declarator = Declarator::Pointer { is_const: false, inner: &Declarator::Abstract };
        Declaration { ty: self.pointee, declarator, is_const: true }.write_to(writer).await;
    }
}

/// A C type name without an identifier, as used in casts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeName<'a> {
    ty: &'a MonoType,
}

impl<'a> TypeName<'a> {
    pub(crate) const fn new(ty: &'a MonoType) -> Self { Self { ty } }
}

impl WriteC for TypeName<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) { self.write_to(writer).await }
}

impl TypeName<'_> {
    async fn write_to(&self, writer: &mut CWriter<'_>) {
        let declarator = Declarator::Abstract;
        Declaration { ty: self.ty, declarator, is_const: false }.write_to(writer).await;
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
}

impl ParameterList<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) {
        let signature = self.signature;
        let mut written = 0;
        match self.parameters {
            // the environment, the source parameters, then the handler
            // records, as `FunctionSignature` orders them in C; a handler
            // record is a pointer, which is never zero-sized
            Parameters::Unnamed => {
                if let Some(slot) = signature.environment_slot() {
                    let environment = slot.ty();
                    if !writer.is_zero_sized(&environment).await {
                        separate(writer, &mut written);
                        TypeName::new(&environment).write_to(writer).await;
                    }
                }
                for parameter in signature.parameter_types() {
                    if !writer.is_zero_sized(parameter).await {
                        separate(writer, &mut written);
                        TypeName::new(parameter).write_to(writer).await;
                    }
                }
                for effect in signature.handler_record_slots() {
                    let record = MonoType::Aggregate(AggregateType::EffectHandlerCallback(
                        EffectHandlerCallback::new(effect.clone()),
                    ));
                    separate(writer, &mut written);
                    ConstPointerTypeName { pointee: &record }.write_to(writer).await;
                }
            }
            // the parameter locals are created in the same order, with the
            // type of each slot
            Parameters::NamedAfter(function) => {
                assert_eq!(signature.abi_parameter_count(), function.abi_parameters().count());
                for local in function.abi_parameters() {
                    let ty = function.get_local(local).ty();
                    if !writer.is_zero_sized(ty).await {
                        separate(writer, &mut written);
                        Declaration::new(ty, &LocalName(local)).write_to(writer).await;
                    }
                }
            }
        }

        // C spells "no parameters" as `void`; `()` would mean "unknown"
        match (signature.is_variadic(), written == 0) {
            (true, true) => writer.push("..."),
            (true, false) => writer.push(", ..."),
            (false, true) => writer.push("void"),
            (false, false) => {}
        }
    }
}

/// Separates an item from the ones written before it with `", "`, counting it
/// in `written`.
fn separate(writer: &mut CWriter<'_>, written: &mut usize) {
    if *written != 0 {
        writer.push(", ");
    }
    *written += 1;
}

/// A function with the given signature, e.g. `int32_t f(int32_t ray_local_0)`.
#[derive(Clone, Copy)]
pub(crate) struct FunctionDeclaration<'a> {
    signature: &'a FunctionSignature,
    name: DeclaredName<'a>,
    parameters: Parameters<'a>,
}

impl<'a> FunctionDeclaration<'a> {
    /// Declares a function called `name`, with unnamed parameters.
    pub(crate) const fn new(signature: &'a FunctionSignature, name: DeclaredName<'a>) -> Self {
        Self { signature, name, parameters: Parameters::Unnamed }
    }

    /// Spells the parameters with the local names of `function`.
    pub(crate) const fn with_parameters_of(mut self, function: &'a MonoFunction) -> Self {
        self.parameters = Parameters::NamedAfter(function);
        self
    }
}

impl WriteC for FunctionDeclaration<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) {
        let function = Declarator::Function {
            inner: &Declarator::Name(self.name),
            signature: self.signature,
            parameters: self.parameters,
        };
        write_function_declaration(writer, self.signature, function).await;
    }
}
