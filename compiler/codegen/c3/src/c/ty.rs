//! Rendering of C types, declarations, and function signatures.
//!
//! C spells a declaration "inside out": a base type followed by a declarator
//! that nests the identifier in pointer and function syntax, e.g.
//! `int32_t *const *x` or `int32_t (*f(void))(int32_t)`. A [`Declarator`] is a
//! stack-allocated linked list of those wrappers, built while walking down a
//! [`MonoType`] and rendered once the base type is reached, so no
//! intermediate strings are allocated.
//!
//! Several identifiers of the same type can share one declaration. The base
//! type is written once, but the declarator is written around each identifier,
//! since in C the `*` and `(...)` belong to the identifier, not to the base:
//! `int32_t *a, *b` declares two pointers, while `int32_t *a, b` would declare
//! `b` an `int32_t`.
//!
//! Spelling a type asks the engine for its representation, since a value
//! that holds nothing is never declared, passed or returned, and a value of a
//! transparent aggregate is spelled as the only member it stores; so the types
//! here implement [`WriteC`] rather than [`Display`].
//!
//! Behind a pointer, an aggregate is always spelled as its own struct, which
//! need not be complete. A transparent aggregate may store a pointer to
//! itself, as `struct Link: next: *mut Link` does, and spelling the pointee
//! as the stored member would never end. A type nests other types,
//! so the calls that can recurse are boxed. The recursion goes through the
//! inherent `write_to` methods, never through [`WriteC`]: an `async fn` of a
//! trait implementation cannot await itself, even boxed.

use std::fmt::Display;

use rayc_mono_ir::{
    function::{LocalID, MonoFunction},
    layout::repr::Repr,
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

/// The identifiers a declaration declares, all of the same type.
#[derive(Clone, Copy)]
enum Identifiers<'a> {
    /// None, as in a type name used by a cast.
    None,
    /// One name.
    Name(DeclaredName<'a>),
    /// At least one local, as in `int32_t ray_local_1, ray_local_2`.
    Locals(&'a [LocalID]),
}

/// The part of a C declaration that surrounds a declared identifier.
#[derive(Clone, Copy)]
enum Declarator<'a> {
    /// Where the identifier goes, if the declaration has one.
    Identifier,
    /// `*inner`, or `*const inner` when the pointer object is `const`.
    Pointer { is_const: bool, inner: &'a Self },
    /// `inner(parameters)`: a function returning the base type.
    Function { inner: &'a Self, signature: &'a FunctionSignature, parameters: Parameters<'a> },
}

impl Declarator<'_> {
    /// Whether the declarator is only the identifier, so a type name, which
    /// has none, writes nothing for it.
    const fn is_identifier(&self) -> bool { matches!(self, Self::Identifier) }

    /// Whether the declared object is a pointer to the base type, which C
    /// lets be incomplete.
    const fn is_pointer(&self) -> bool { matches!(self, Self::Pointer { .. }) }

    /// Writes the declarator around `identifier`, or around nothing in a type
    /// name.
    async fn write_c(&self, writer: &mut CWriter<'_>, identifier: Option<DeclaredName<'_>>) {
        match self {
            Self::Identifier => {
                if let Some(identifier) = identifier {
                    writer.push(identifier);
                }
            }
            Self::Pointer { is_const, inner } => {
                writer.push("*");
                if *is_const {
                    writer.push("const");
                    if identifier.is_some() || !inner.is_identifier() {
                        writer.push(" ");
                    }
                }
                Box::pin(inner.write_c(writer, identifier)).await;
            }
            // a postfix `(...)` binds tighter than a prefix `*`, so a pointer
            // being called through needs parentheses: `(*f)(int32_t)`
            Self::Function { inner, signature, parameters } => {
                match inner {
                    Self::Pointer { .. } => {
                        writer.push("(");
                        Box::pin(inner.write_c(writer, identifier)).await;
                        writer.push(")");
                    }
                    Self::Identifier | Self::Function { .. } => {
                        Box::pin(inner.write_c(writer, identifier)).await;
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

/// Writes `base declarator`, qualifying the base when the declared objects
/// are `const`, with the declarator written around each of the `identifiers`
/// in turn.
async fn write_declaration(
    writer: &mut CWriter<'_>,
    base: impl Display,
    is_const: bool,
    declarator: &Declarator<'_>,
    identifiers: Identifiers<'_>,
) {
    if is_const {
        writer.push("const ");
    }
    writer.push(base);

    match identifiers {
        // a type name with nothing around its missing identifier is the base
        // alone, as in `(int32_t)x`
        Identifiers::None => {
            if !declarator.is_identifier() {
                writer.push(" ");
                declarator.write_c(writer, None).await;
            }
        }
        Identifiers::Name(name) => {
            writer.push(" ");
            declarator.write_c(writer, Some(name)).await;
        }
        Identifiers::Locals(locals) => {
            for (index, local) in locals.iter().enumerate() {
                writer.push(if index == 0 { " " } else { ", " });
                declarator.write_c(writer, Some(&LocalName(*local))).await;
            }
        }
    }
}

/// Writes a function returning the signature's return type, where
/// `declarator` is a [`Declarator::Function`]. A function whose result is
/// zero-sized returns `void`.
async fn write_function_declaration(
    writer: &mut CWriter<'_>,
    signature: &FunctionSignature,
    declarator: Declarator<'_>,
    identifiers: Identifiers<'_>,
) {
    match signature.return_type() {
        ReturnType::Value(ty) if !writer.is_zero_sized(ty).await => {
            let declaration = Declaration { ty, declarator, identifiers, is_const: false };
            Box::pin(declaration.write_to(writer)).await;
        }
        ReturnType::Void | ReturnType::Value(_) => {
            write_declaration(writer, "void", false, &declarator, identifiers).await;
        }
    }
}

/// A C declaration of `identifiers` with type `ty`, e.g.
/// `int32_t ray_local_0`.
#[derive(Clone, Copy)]
pub(crate) struct Declaration<'a> {
    ty: &'a MonoType,
    declarator: Declarator<'a>,
    identifiers: Identifiers<'a>,
    /// Whether the declared objects themselves are `const`.
    is_const: bool,
}

impl<'a> Declaration<'a> {
    /// Declares `name`, whose type must not be zero-sized: no object of such
    /// a type is ever declared.
    pub(crate) const fn new(ty: &'a MonoType, name: DeclaredName<'a>) -> Self {
        Self::of(ty, Identifiers::Name(name))
    }

    /// Declares every one of `locals` in one declaration, as locals of type
    /// `ty`, which must not be zero-sized.
    pub(crate) fn locals(ty: &'a MonoType, locals: &'a [LocalID]) -> Self {
        assert!(!locals.is_empty(), "a declaration should declare at least one local");
        Self::of(ty, Identifiers::Locals(locals))
    }

    const fn of(ty: &'a MonoType, identifiers: Identifiers<'a>) -> Self {
        Self { ty, declarator: Declarator::Identifier, identifiers, is_const: false }
    }

    async fn write_base(&self, writer: &mut CWriter<'_>, base: impl Display) {
        write_declaration(writer, base, self.is_const, &self.declarator, self.identifiers).await;
    }

    /// Writes a pointer to the named `pointee` base type.
    async fn write_pointer_to(&self, writer: &mut CWriter<'_>, pointee: &str) {
        let declarator = self.pointer_declarator();
        write_declaration(writer, pointee, false, &declarator, self.identifiers).await;
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
    /// Writes the declaration of a value of `aggregate`, the type of the
    /// declaration, or of a pointer to one.
    ///
    /// This is the only place an aggregate is spelled as its struct, so the
    /// only place it is recorded. Only a direct aggregate is defined; any
    /// other is spelled as its struct only behind a pointer, which C lets be
    /// incomplete.
    async fn write_aggregate(&self, writer: &mut CWriter<'_>, aggregate: &AggregateType) {
        if !self.declarator.is_pointer() {
            match writer.repr(self.ty).await {
                Repr::ZeroSized => panic!("a zero-sized type is only spelled behind a pointer"),
                // the value is the member it stores, declared the same way
                Repr::Transparent(inner) => {
                    let inner = Declaration { ty: &inner, ..*self };
                    Box::pin(inner.write_to(writer)).await;
                    return;
                }
                Repr::Direct => {}
            }
        }

        writer.record_aggregate(aggregate);
        self.write_base(writer, AggregateName::of(aggregate).typedef()).await;
    }
}

impl Declaration<'_> {
    async fn write_to(&self, writer: &mut CWriter<'_>) {
        match self.ty {
            MonoType::Int8 => self.write_base(writer, "int8_t").await,
            MonoType::Int16 => self.write_base(writer, "int16_t").await,
            MonoType::Int32 => self.write_base(writer, "int32_t").await,
            MonoType::Int64 => self.write_base(writer, "int64_t").await,
            MonoType::Isize => self.write_base(writer, "intptr_t").await,
            // a `bool` is a byte rather than a C `bool`, which would turn any
            // value other than 0 into 1: an enclosing enum may store its
            // discriminant in the values from 2 on
            MonoType::Bool | MonoType::Uint8 => self.write_base(writer, "uint8_t").await,
            MonoType::Uint16 => self.write_base(writer, "uint16_t").await,
            MonoType::Uint32 => self.write_base(writer, "uint32_t").await,
            MonoType::Uint64 => self.write_base(writer, "uint64_t").await,
            MonoType::Usize => self.write_base(writer, "uintptr_t").await,
            MonoType::Float32 => self.write_base(writer, "float").await,
            MonoType::CInt => self.write_base(writer, "int").await,
            MonoType::Aggregate(aggregate) => self.write_aggregate(writer, aggregate).await,

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
                    identifiers: self.identifiers,
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
                write_function_declaration(writer, signature, function, self.identifiers).await;
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
        let declarator = Declarator::Pointer { is_const: false, inner: &Declarator::Identifier };
        let identifiers = Identifiers::None;
        Declaration { ty: self.pointee, declarator, identifiers, is_const: true }
            .write_to(writer)
            .await;
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
        Declaration::of(self.ty, Identifiers::None).write_to(writer).await;
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
            inner: &Declarator::Identifier,
            signature: self.signature,
            parameters: self.parameters,
        };
        let identifiers = Identifiers::Name(self.name);
        write_function_declaration(writer, self.signature, function, identifiers).await;
    }
}
