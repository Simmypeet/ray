use rayc_mono_ir::ty::{FunctionSignature, MonoType, PointerMutability, ReturnType};

use crate::name::aggregate_typedef_name;

pub(super) fn type_name(ty: &MonoType) -> String {
    declaration_with_constness(ty, "", false).trim_end().to_owned()
}

pub(super) fn declaration(ty: &MonoType, name: &str) -> String {
    declaration_with_constness(ty, name, false)
}

fn declaration_with_constness(ty: &MonoType, name: &str, is_const: bool) -> String {
    match ty {
        MonoType::Bool => base_declaration("bool", name, is_const),
        MonoType::Int8 => base_declaration("int8_t", name, is_const),
        MonoType::Int16 => base_declaration("int16_t", name, is_const),
        MonoType::Int32 => base_declaration("int32_t", name, is_const),
        MonoType::Int64 => base_declaration("int64_t", name, is_const),
        MonoType::Isize => base_declaration("intptr_t", name, is_const),
        MonoType::Uint8 => base_declaration("uint8_t", name, is_const),
        MonoType::Uint16 => base_declaration("uint16_t", name, is_const),
        MonoType::Uint32 => base_declaration("uint32_t", name, is_const),
        MonoType::Uint64 => base_declaration("uint64_t", name, is_const),
        MonoType::Usize => base_declaration("uintptr_t", name, is_const),
        MonoType::Float32 => base_declaration("float", name, is_const),
        MonoType::CInt => base_declaration("int", name, is_const),
        MonoType::CStr => pointer_declaration("const char", name, is_const),
        MonoType::OpaquePointer(mutability) => {
            let base = match mutability {
                PointerMutability::Const => "const void",
                PointerMutability::Mut => "void",
            };
            pointer_declaration(base, name, is_const)
        }
        MonoType::Pointer(pointer) => {
            let pointer_name =
                if is_const { format!("* const {name}") } else { format!("*{name}") };
            declaration_with_constness(
                pointer.pointee(),
                &pointer_name,
                pointer.mutability() == PointerMutability::Const,
            )
        }
        MonoType::Aggregate(aggregate) => {
            base_declaration(&aggregate_typedef_name(aggregate), name, is_const)
        }
        MonoType::FunctionPointer(signature) => {
            let pointer_name =
                if is_const { format!("(* const {name})") } else { format!("(*{name})") };
            signature_declaration(signature, &pointer_name, None)
        }
    }
}

fn base_declaration(base: &str, name: &str, is_const: bool) -> String {
    if is_const { format!("const {base} {name}") } else { format!("{base} {name}") }
}

fn pointer_declaration(base: &str, name: &str, is_const: bool) -> String {
    if is_const { format!("{base} * const {name}") } else { format!("{base} *{name}") }
}

pub(super) fn signature_declaration(
    signature: &FunctionSignature,
    name: &str,
    parameter_names: Option<&[String]>,
) -> String {
    let return_type = match signature.return_type() {
        ReturnType::Void => "void".to_owned(),
        ReturnType::Value(return_type) => type_name(return_type),
    };

    let mut parameters = signature
        .parameter_types()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            parameter_names
                .map_or_else(|| type_name(parameter), |names| declaration(parameter, &names[index]))
        })
        .collect::<Vec<_>>();
    if signature.is_variadic() {
        parameters.push("...".to_owned());
    } else if parameters.is_empty() {
        parameters.push("void".to_owned());
    }

    format!("{return_type} {name}({})", parameters.join(", "))
}

/// Returns whether C's integer promotions turn an unsigned value of this type
/// into a signed `int`.
pub(super) const fn promotes_to_signed_int(ty: &MonoType) -> bool {
    match ty {
        MonoType::Uint8 | MonoType::Uint16 => true,
        MonoType::Bool
        | MonoType::Int8
        | MonoType::Int16
        | MonoType::Int32
        | MonoType::Int64
        | MonoType::Isize
        | MonoType::Uint32
        | MonoType::Uint64
        | MonoType::Usize
        | MonoType::Float32
        | MonoType::CInt
        | MonoType::CStr
        | MonoType::OpaquePointer(_)
        | MonoType::Pointer(_)
        | MonoType::Aggregate(_)
        | MonoType::FunctionPointer(_) => false,
    }
}
