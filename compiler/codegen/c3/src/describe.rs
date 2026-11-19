//! Ray-level descriptions of generated C items, such as
//! `main.foo[int32, bool]`, which are written in a comment above each item so
//! that the hashed C names can be told apart when reading the output.
//!
//! Lifetime arguments are left out: every lifetime is erased by the time code
//! is generated, so they would only say `'_`.

use std::{
    fmt::{Display, Write as _},
    pin::Pin,
};

use rayc_ir::{get_ir, ir_function::IRContext};
use rayc_mono_ir::{
    MonoBodyInstance, MonoFunctionInstance,
    ty::{AggregateType, EffectRow, MonoType, PointerMutability, ReturnType},
};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_qualified_name};
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::Subst,
    ty::{Ty, TyKind},
};

const WRITE_TO_STRING: &str = "writing to a String cannot fail";

/// Describes the function `instance`.
pub(crate) async fn describe_function(
    engine: &TrackedEngine,
    instance: &MonoFunctionInstance,
) -> String {
    let mut describer = Describer { engine, out: String::new() };
    describer.function(instance).await;
    describer.out
}

/// Describes the aggregate type `aggregate`.
pub(crate) async fn describe_aggregate(
    engine: &TrackedEngine,
    aggregate: &AggregateType,
) -> String {
    let mut describer = Describer { engine, out: String::new() };
    describer.aggregate(aggregate).await;
    describer.out
}

/// Writes a description into a buffer, querying the names it needs.
struct Describer<'e> {
    engine: &'e TrackedEngine,
    out: String,
}

/// The future of a description that may describe itself again, such as a
/// tuple containing a tuple, which must be boxed to have a known size.
type Recursion<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

impl Describer<'_> {
    fn write(&mut self, text: impl Display) { write!(self.out, "{text}").expect(WRITE_TO_STRING); }

    async fn function(&mut self, instance: &MonoFunctionInstance) {
        match instance {
            MonoFunctionInstance::Body(body) => self.body(body).await,
            MonoFunctionInstance::ExternDef(def_id) => {
                self.write("extern ");
                self.symbol(*def_id).await;
            }
            MonoFunctionInstance::NominalDrop(instance) => {
                self.write("drop of ");
                self.semantic_type(instance.view().nominal()).await;
            }
            MonoFunctionInstance::CoroutineResume(coroutine) => {
                self.write("resume of coroutine ");
                self.body(coroutine.body()).await;
            }
            MonoFunctionInstance::CoroutineDrop(coroutine) => {
                self.write("drop of coroutine ");
                self.body(coroutine.body()).await;
            }
        }
    }

    /// The function of a definition, or of a closure or a handler clause
    /// written in it.
    async fn body(&mut self, body: &MonoBodyInstance) {
        let ir = self.engine.get_ir(body.def_id()).await;
        let index = body.ir_function_id().index();
        match ir.get_function(body.ir_function_id()).context() {
            IRContext::Def => {}
            IRContext::Closure(_) => self.write(format_args!("closure #{index} in ")),
            IRContext::Thunk(_) => self.write(format_args!("thunk #{index} in ")),
            IRContext::OperationHandler(handler) => {
                self.write("handler of ");
                self.symbol(handler.operation()).await;
                self.write(" in ");
            }
        }
        self.application(body.def_id(), body.substitution()).await;
    }

    /// `symbol[arguments]`: the arguments `substitution` gives the variables
    /// of `symbol` and its enclosing symbols, without the lifetimes, and with
    /// the instances after `; given`.
    async fn application(&mut self, symbol: GlobalSymbolID, substitution: &Subst) {
        self.symbol(symbol).await;

        let poly_vars = self.engine.get_enclosing_poly_var_maps(symbol).await;
        let mut types = Vec::new();
        let mut instances = Vec::new();
        for (poly_var_id, poly_var) in poly_vars.iter() {
            let argument =
                substitution.get(&poly_var_id).expect("a mono instance gives every variable");

            match poly_var.kind() {
                TyKind::Lifetime => {}
                TyKind::Instance => instances.push(argument),
                TyKind::Star | TyKind::EffectRow2 | TyKind::EffectRow => types.push(argument),
            }
        }
        if types.is_empty() && instances.is_empty() {
            return;
        }

        self.write("[");
        for (index, argument) in types.iter().enumerate() {
            if index != 0 {
                self.write(", ");
            }
            self.semantic_type(argument).await;
        }
        if !instances.is_empty() {
            self.write(if types.is_empty() { "given " } else { "; given " });
        }
        for (index, argument) in instances.iter().enumerate() {
            if index != 0 {
                self.write(", ");
            }
            self.semantic_type(argument).await;
        }
        self.write("]");
    }

    /// The qualified name of `symbol`, such as `module.def`.
    async fn symbol(&mut self, symbol: GlobalSymbolID) {
        let name = self.engine.get_qualified_name(symbol).await;
        self.write(name);
    }

    /// A type argument, as diagnostics spell it but without lifetimes.
    async fn semantic_type(&mut self, ty: &Ty) {
        let display = ty.display(self.engine).await.without_erased_lifetimes();
        self.write(display);
    }

    fn aggregate<'a>(&'a mut self, aggregate: &'a AggregateType) -> Recursion<'a> {
        Box::pin(async move {
            match aggregate {
                AggregateType::Tuple(tuple) => {
                    self.write("(");
                    for (index, field) in tuple.fields().iter().enumerate() {
                        if index != 0 {
                            self.write(", ");
                        }
                        self.mono_type(field).await;
                    }
                    // a one-element tuple keeps its comma, as in Ray
                    self.write(if tuple.fields().len() == 1 { ",)" } else { ")" });
                }
                AggregateType::Struct(instance) => {
                    self.application(instance.struct_id(), instance.substitution()).await;
                }
                AggregateType::Enum(instance) => {
                    self.application(instance.enum_id(), instance.substitution()).await;
                }
                AggregateType::Environment(instance) => {
                    let capture_map = instance.capture_map().index();
                    self.write(format_args!("captures #{capture_map} in "));
                    self.application(instance.def_id(), instance.substitution()).await;
                }
                AggregateType::EffectHandlerCallback(handler) => {
                    self.write("handler record of ");
                    let effect = handler.mono_effect_instance();
                    self.application(effect.effect_id(), effect.substitution()).await;
                }
                AggregateType::CoroutineFrame(coroutine) => {
                    self.write("frame of coroutine ");
                    self.body(coroutine.body()).await;
                }
                AggregateType::CoroutineResume(resume) => {
                    self.write("resume value of ");
                    self.row(resume.row()).await;
                }
                AggregateType::CoroutineYield(yielded) => {
                    self.write("yield of ");
                    self.row(yielded.row()).await;
                }
                AggregateType::CoroutineProgress(progress) => {
                    self.write("progress of ");
                    self.row(progress.row()).await;
                    self.write(" returning ");
                    self.mono_type(progress.return_type()).await;
                }
                AggregateType::RunWithFrames(instance) => {
                    let expression = instance.expression().index();
                    self.write(format_args!("frames of run ... with #{expression} in "));
                    self.body(instance.owner()).await;
                }
            }
        })
    }

    /// An effect row, such as `{main.Read[int32], main.Log}`.
    async fn row(&mut self, row: &EffectRow) {
        self.write("{");
        for (index, effect) in row.effects().iter().enumerate() {
            if index != 0 {
                self.write(", ");
            }
            self.application(effect.effect_id(), effect.substitution()).await;
        }
        self.write("}");
    }

    /// A type as generated code holds it, spelled as close to Ray as it can
    /// be.
    async fn mono_type(&mut self, ty: &MonoType) {
        match ty {
            MonoType::Bool => self.write("bool"),
            MonoType::Int8 => self.write("int8"),
            MonoType::Int16 => self.write("int16"),
            MonoType::Int32 => self.write("int32"),
            MonoType::Int64 => self.write("int64"),
            MonoType::Isize => self.write("isize"),
            MonoType::Uint8 => self.write("uint8"),
            MonoType::Uint16 => self.write("uint16"),
            MonoType::Uint32 => self.write("uint32"),
            MonoType::Uint64 => self.write("uint64"),
            MonoType::Usize => self.write("usize"),
            MonoType::Float32 => self.write("float32"),
            MonoType::CInt => self.write("c_int"),
            MonoType::CStr => self.write("cstr"),
            MonoType::OpaquePointer(mutability) => {
                self.write(pointer_prefix(*mutability));
                self.write("opaque");
            }
            MonoType::Pointer(pointer) => {
                self.write(pointer_prefix(pointer.mutability()));
                Box::pin(self.mono_type(pointer.pointee())).await;
            }
            MonoType::Aggregate(aggregate) => self.aggregate(aggregate).await,
            MonoType::FunctionPointer(signature) => {
                self.write("fn(");
                for (index, parameter) in signature.parameter_types().iter().enumerate() {
                    if index != 0 {
                        self.write(", ");
                    }
                    Box::pin(self.mono_type(parameter)).await;
                }
                self.write(")");
                match signature.return_type() {
                    ReturnType::Void => {}
                    ReturnType::Value(ty) => {
                        self.write(" -> ");
                        Box::pin(self.mono_type(ty)).await;
                    }
                }
            }
        }
    }
}

/// `*` or `*mut `, as Ray spells a pointer of `mutability`.
const fn pointer_prefix(mutability: PointerMutability) -> &'static str {
    match mutability {
        PointerMutability::Const => "*",
        PointerMutability::Mut => "*mut ",
    }
}
