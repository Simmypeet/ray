use std::fmt::Write;

use rayc_mono_ir::{
    MonoIR,
    cfg::Terminator,
    function::{LocalKind, MonoFunction, MonoFunctionID},
    instruction::{Call, Instruction},
    operand::{Constant, Operand},
    place::{Place, Projection},
    rvalue::{AggregateEffectHandler, AggregateValue, BinaryOperator, Rvalue, UnaryOperator},
    ty::{
        AggregateType, FunctionSignature, HandlerLayout, MonoType, ReturnType, build_handler_layout,
    },
};

use crate::{
    c_type::{declaration, promotes_to_signed_int, signature_declaration, type_name},
    generator::Generator,
    name::{
        aggregate_name, aggregate_typedef_name, block_name, environment_field_name,
        ir_function_name, local_name, operation_environment_field_name,
        operation_function_field_name, struct_field_name, tuple_field_name,
    },
};

impl Generator<'_> {
    pub(super) async fn emit_function_definition(
        &self,
        ir: &MonoIR,
        function_id: MonoFunctionID,
    ) -> String {
        let function = ir.get_function(function_id);
        let name = ir_function_name(ir, function_id);
        let parameter_names =
            function.parameters().map(|local| local_name(local.index())).collect::<Vec<_>>();
        let mut output = signature_declaration(function.signature(), &name, Some(&parameter_names));
        output.push_str(" {\n");

        for (local_id, local) in function.locals() {
            match local.kind() {
                LocalKind::Parameter => {}
                LocalKind::Variable | LocalKind::Temporary => {
                    writeln!(
                        output,
                        "    {};",
                        declaration(local.ty(), &local_name(local_id.index()))
                    )
                    .unwrap();
                }
            }
        }
        if function.locals().any(|(_, local)| local.kind() != LocalKind::Parameter) {
            output.push('\n');
        }

        let mut blocks = function.blocks().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|(block_id, _)| {
            (*block_id != function.entry_block(), block_id.index())
        });
        for (block_id, block) in blocks {
            writeln!(output, "{}:", block_name(block_id.index())).unwrap();
            for instruction in block.instructions() {
                let instruction = self.emit_instruction(instruction, ir, function).await;
                writeln!(output, "    {instruction}").unwrap();
            }
            let terminator = self
                .emit_terminator(
                    block.terminator().expect("reachable MonoIR block should have a terminator"),
                    ir,
                    function,
                )
                .await;
            writeln!(output, "    {terminator}").unwrap();
        }

        output.push('}');
        output
    }

    pub(super) fn emit_aggregate_definition(
        aggregate: &AggregateType,
        handler_layout: Option<&HandlerLayout>,
    ) -> String {
        let mut output = format!("struct {} {{\n", aggregate_name(aggregate));
        match aggregate {
            AggregateType::EffectHandler(_) => {
                let handler_layout = handler_layout
                    .expect("an effect-handler aggregate should have a resolved handler layout");
                if handler_layout.operations().is_empty() {
                    output.push_str("    uint8_t _unit;\n");
                }
                for operation in handler_layout.operations() {
                    let signature = operation.signature();
                    let environment_type = signature
                        .parameter_types()
                        .first()
                        .expect("an operation signature should have an environment parameter");
                    writeln!(
                        output,
                        "    {};",
                        declaration(
                            environment_type,
                            &operation_environment_field_name(operation.operation_id()),
                        )
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "    {};",
                        signature_declaration(
                            signature,
                            &format!(
                                "(*{})",
                                operation_function_field_name(operation.operation_id())
                            ),
                            None,
                        )
                    )
                    .unwrap();
                }
            }
            AggregateType::Tuple(tuple) => {
                if tuple.is_empty() {
                    output.push_str("    uint8_t _unit;\n");
                }
                for (index, field) in tuple.fields().iter().enumerate() {
                    writeln!(
                        output,
                        "    {};",
                        declaration(field, &tuple_field_name(index.try_into().unwrap()))
                    )
                    .unwrap();
                }
            }
            AggregateType::Environment(environment) => {
                if environment.captures().is_empty() {
                    output.push_str("    uint8_t _unit;\n");
                }
                for (index, capture) in environment.captures().iter().enumerate() {
                    writeln!(
                        output,
                        "    {};",
                        declaration(capture, &environment_field_name(index.try_into().unwrap()))
                    )
                    .unwrap();
                }
            }
            AggregateType::Struct(st) => {
                if st.fields().is_empty() {
                    output.push_str("    uint8_t _unit;\n");
                }
                for (field_id, field_type) in st.fields() {
                    writeln!(
                        output,
                        "    {};",
                        declaration(field_type, &struct_field_name(*field_id))
                    )
                    .unwrap();
                }
            }
        }
        output.push_str("};");
        output
    }

    async fn emit_instruction(
        &self,
        instruction: &Instruction,
        ir: &MonoIR,
        function: &MonoFunction,
    ) -> String {
        match instruction {
            Instruction::Assign(assign) => {
                let destination_type = self.place_type(assign.destination(), function).await;
                let destination = place_expression(assign.destination());
                let value =
                    self.emit_rvalue(assign.value(), ir, function, Some(&destination_type)).await;
                format!("{destination} = {value};")
            }
            Instruction::Call(call) => self.emit_call(call, ir, function).await,
        }
    }

    async fn emit_call(&self, call: &Call, ir: &MonoIR, function: &MonoFunction) -> String {
        let signature = match call.callee() {
            Operand::Function(callee) => callee.signature().clone(),
            Operand::Copy(place) => {
                let ty = self.place_type(place, function).await;
                let MonoType::FunctionPointer(signature) = ty else {
                    panic!("an indirect call callee should have function-pointer type")
                };
                signature
            }
            Operand::Constant(_) => {
                panic!("a constant cannot be used as a MonoIR call target")
            }
        };

        let callee = self.emit_operand(call.callee(), ir, function, None).await;
        let mut arguments = Vec::with_capacity(call.arguments().len());
        for (index, argument) in call.arguments().iter().enumerate() {
            arguments.push(
                self.emit_operand(
                    argument,
                    ir,
                    function,
                    signature.parameter_types().get(index).map(AsRef::as_ref),
                )
                .await,
            );
        }
        if !signature.is_variadic() {
            assert_eq!(
                call.arguments().len(),
                signature.parameter_types().len(),
                "a non-variadic MonoIR call should match its signature"
            );
        }

        let invocation = format!("{callee}({})", arguments.join(", "));
        call.destination().map_or_else(
            || format!("{invocation};"),
            |destination| format!("{} = {invocation};", place_expression(destination)),
        )
    }

    async fn emit_terminator(
        &self,
        terminator: &Terminator,
        ir: &MonoIR,
        function: &MonoFunction,
    ) -> String {
        match terminator {
            Terminator::Goto(target) => format!("goto {};", block_name(target.index())),
            Terminator::Branch(branch) => {
                let condition = self
                    .emit_operand(branch.condition(), ir, function, Some(&MonoType::Bool))
                    .await;
                format!(
                    "if ({condition}) goto {}; else goto {};",
                    block_name(branch.then_block().index()),
                    block_name(branch.else_block().index())
                )
            }
            Terminator::Return(value) => match value {
                Some(value) => {
                    let return_type = match function.signature().return_type() {
                        ReturnType::Void => {
                            panic!("a void MonoIR return cannot carry an operand")
                        }
                        ReturnType::Value(return_type) => return_type,
                    };
                    let value = self.emit_operand(value, ir, function, Some(return_type)).await;
                    format!("return {value};")
                }
                None => "return;".to_owned(),
            },
            Terminator::Unreachable => "__builtin_unreachable();".to_owned(),
        }
    }

    async fn emit_rvalue(
        &self,
        value: &Rvalue,
        ir: &MonoIR,
        function: &MonoFunction,
        expected_type: Option<&MonoType>,
    ) -> String {
        match value {
            Rvalue::Use(operand) => self.emit_operand(operand, ir, function, expected_type).await,
            Rvalue::AddressOf(address) => format!("&({})", place_expression(address.place())),
            Rvalue::Unary(unary) => {
                let operator = match unary.operator() {
                    UnaryOperator::Negate => "-",
                    UnaryOperator::LogicalNot => "!",
                    UnaryOperator::BitwiseNot => "~",
                };
                let operand = self.emit_operand(unary.operand(), ir, function, None).await;
                format!("({operator}{operand})")
            }
            Rvalue::Binary(binary) => {
                let operator = match binary.operator() {
                    BinaryOperator::Equal => "==",
                    BinaryOperator::NotEqual => "!=",
                    BinaryOperator::Less => "<",
                    BinaryOperator::LessEqual => "<=",
                    BinaryOperator::Greater => ">",
                    BinaryOperator::GreaterEqual => ">=",
                    BinaryOperator::Add => "+",
                    BinaryOperator::Subtract => "-",
                    BinaryOperator::Multiply => "*",
                    BinaryOperator::Divide => "/",
                    BinaryOperator::Remainder => "%",
                    BinaryOperator::LogicalAnd => "&&",
                    BinaryOperator::LogicalOr => "||",
                    BinaryOperator::BitwiseAnd => "&",
                    BinaryOperator::BitwiseOr => "|",
                    BinaryOperator::BitwiseXor => "^",
                    BinaryOperator::ShiftLeft => "<<",
                    BinaryOperator::ShiftRight => ">>",
                };
                let left = self.emit_operand(binary.left(), ir, function, None).await;
                let right = self.emit_operand(binary.right(), ir, function, None).await;

                // C promotes `uint8` and `uint16` operands to a signed `int`,
                // whose multiplication can overflow, e.g. `65535u16 * 65535u16`.
                // Multiplying as `uint32_t` wraps instead, and the assignment
                // truncates the product back to the operand type.
                if matches!(binary.operator(), BinaryOperator::Multiply)
                    && expected_type.is_some_and(promotes_to_signed_int)
                {
                    return format!("((uint32_t){left} * (uint32_t){right})");
                }

                format!("({left} {operator} {right})")
            }
            Rvalue::Cast(cast) => {
                let operand = self.emit_operand(cast.operand(), ir, function, None).await;
                format!("(({})({operand}))", type_name(cast.target()))
            }
            Rvalue::Aggregate(aggregate) => {
                self.emit_aggregate_value(aggregate, ir, function).await
            }
        }
    }

    async fn emit_aggregate_value(
        &self,
        aggregate: &AggregateValue,
        ir: &MonoIR,
        function: &MonoFunction,
    ) -> String {
        let (ty, fields) = match aggregate {
            AggregateValue::Tuple(tuple) => {
                let ty = AggregateType::Tuple(tuple.ty().clone());
                if tuple.fields().is_empty() {
                    (ty, vec!["._unit = 0".to_owned()])
                } else {
                    let mut fields = Vec::with_capacity(tuple.fields().len());
                    for (index, (field, field_type)) in
                        tuple.fields().iter().zip(tuple.ty().fields()).enumerate()
                    {
                        let value = self.emit_operand(field, ir, function, Some(field_type)).await;
                        fields.push(format!(
                            ".{} = {value}",
                            tuple_field_name(index.try_into().unwrap())
                        ));
                    }
                    (ty, fields)
                }
            }
            AggregateValue::Environment(environment) => {
                let ty = AggregateType::Environment(environment.ty().clone());
                let mut fields = Vec::with_capacity(environment.fields().len());
                for (index, (field, field_type)) in
                    environment.fields().iter().zip(environment.ty().captures()).enumerate()
                {
                    let value = self.emit_operand(field, ir, function, Some(field_type)).await;
                    fields.push(format!(
                        ".{} = {value}",
                        environment_field_name(index.try_into().unwrap())
                    ));
                }
                if fields.is_empty() {
                    fields.push("._unit = 0".to_owned());
                }
                (ty, fields)
            }

            AggregateValue::EffectHandler(handler) => {
                self.emit_effect_handler_value(handler, ir, function).await
            }

            AggregateValue::Struct(st) => {
                let ty = AggregateType::Struct(st.ty().clone());
                let mut fields = Vec::with_capacity(st.fields().len());

                for (field_id, field_ty) in st.ty().fields() {
                    let field_value = st
                        .fields()
                        .get(field_id)
                        .expect("struct aggregate should initialize every field");
                    let value = self.emit_operand(field_value, ir, function, Some(field_ty)).await;
                    fields.push(format!(".{} = {value}", struct_field_name(*field_id)));
                }

                if fields.is_empty() {
                    fields.push("._unit = 0".to_owned());
                }
                (ty, fields)
            }
        };
        format!("(({}){{ {} }})", aggregate_typedef_name(&ty), fields.join(", "))
    }

    async fn emit_effect_handler_value(
        &self,
        handler: &AggregateEffectHandler,
        ir: &MonoIR,
        function: &MonoFunction,
    ) -> (AggregateType, Vec<String>) {
        let ty = AggregateType::EffectHandler(rayc_mono_ir::ty::EffectHandler::new(
            handler.effect().clone(),
        ));
        let layout = self.engine().build_handler_layout(handler.effect().clone()).await;
        let mut fields = Vec::with_capacity(handler.slots().len() * 2);
        for operation in layout.operations() {
            let slot = handler
                .slots()
                .get(&operation.operation_id())
                .expect("effect-handler aggregate should initialize every operation slot");
            let environment = self
                .emit_operand(
                    slot.environment(),
                    ir,
                    function,
                    operation.signature().parameter_types().first().map(AsRef::as_ref),
                )
                .await;
            let function_pointer = self.emit_operand(slot.function(), ir, function, None).await;
            fields.push(format!(
                ".{} = {environment}",
                operation_environment_field_name(operation.operation_id())
            ));
            fields.push(format!(
                ".{} = {function_pointer}",
                operation_function_field_name(operation.operation_id())
            ));
        }
        if fields.is_empty() {
            fields.push("._unit = 0".to_owned());
        }
        (ty, fields)
    }

    async fn emit_operand(
        &self,
        operand: &Operand,
        ir: &MonoIR,
        _function: &MonoFunction,
        expected_type: Option<&MonoType>,
    ) -> String {
        match operand {
            Operand::Copy(place) => place_expression(place),
            Operand::Constant(constant) => emit_constant(constant, expected_type),
            Operand::Function(function_operand) => {
                self.callable_name(function_operand.function(), ir).await
            }
        }
    }

    async fn place_type(&self, place: &Place, function: &MonoFunction) -> MonoType {
        let mut ty = (**function.get_local(place.local()).ty()).clone();
        for projection in place.projections() {
            ty = match projection {
                Projection::Dereference => {
                    let MonoType::Pointer(pointer) = ty else {
                        panic!("MonoIR dereference projection requires a pointer")
                    };
                    (**pointer.pointee()).clone()
                }
                Projection::EnvironmentFieldIndex(index) => {
                    let MonoType::Aggregate(AggregateType::Environment(environment)) = ty else {
                        panic!("environment field projection requires an environment aggregate")
                    };
                    (**environment
                        .captures()
                        .get(index.index() as usize)
                        .expect("environment projection index should be in bounds"))
                    .clone()
                }
                Projection::TupleFieldIndex(index) => {
                    let MonoType::Aggregate(AggregateType::Tuple(tuple)) = ty else {
                        panic!("tuple field projection requires a tuple aggregate")
                    };
                    (**tuple
                        .fields()
                        .get(index.index() as usize)
                        .expect("tuple projection index should be in bounds"))
                    .clone()
                }

                Projection::OperationRecordEnvironmentField(operation_id) => {
                    let operation = self.operation_signature(&ty, *operation_id).await;
                    (**operation
                        .parameter_types()
                        .first()
                        .expect("operation signature should have an environment parameter"))
                    .clone()
                }
                Projection::OperationRecordFunctionPointerField(operation_id) => {
                    MonoType::FunctionPointer(self.operation_signature(&ty, *operation_id).await)
                }

                Projection::StructFieldIndex(st) => {
                    let MonoType::Aggregate(AggregateType::Struct(struct_ty)) = ty else {
                        panic!("struct field projection requires a struct aggregate")
                    };
                    (**struct_ty
                        .fields()
                        .get(st)
                        .expect("struct projection field should be in bounds"))
                    .clone()
                }
            };
        }
        ty
    }

    async fn operation_signature(
        &self,
        handler_type: &MonoType,
        operation_id: rayc_symbol::GlobalSymbolID,
    ) -> FunctionSignature {
        let MonoType::Aggregate(AggregateType::EffectHandler(handler)) = handler_type else {
            panic!("operation projection requires an effect-handler aggregate")
        };
        let layout =
            self.engine().build_handler_layout(handler.mono_effect_instance().clone()).await;
        layout
            .operations()
            .iter()
            .find(|operation| operation.operation_id() == operation_id)
            .unwrap_or_else(|| panic!("operation is absent from its concrete handler layout"))
            .signature()
            .clone()
    }
}

fn place_expression(place: &Place) -> String {
    let mut expression = local_name(place.local().index());
    for projection in place.projections() {
        expression = match projection {
            Projection::Dereference => format!("(*({expression}))"),
            Projection::EnvironmentFieldIndex(index) => {
                format!("({expression}).{}", environment_field_name(index.index()))
            }
            Projection::TupleFieldIndex(index) => {
                format!("({expression}).{}", tuple_field_name(index.index()))
            }
            Projection::OperationRecordEnvironmentField(operation) => {
                format!("({expression}).{}", operation_environment_field_name(*operation))
            }
            Projection::OperationRecordFunctionPointerField(operation) => {
                format!("({expression}).{}", operation_function_field_name(*operation))
            }
            Projection::StructFieldIndex(field) => {
                format!("({expression}).{}", struct_field_name(*field))
            }
        };
    }
    expression
}

fn emit_constant(constant: &Constant, expected_type: Option<&MonoType>) -> String {
    match constant {
        Constant::Unit => {
            let Some(MonoType::Aggregate(AggregateType::Tuple(tuple))) = expected_type else {
                panic!("a unit constant requires its expected tuple type during C emission")
            };
            assert!(tuple.is_empty(), "a unit constant requires an empty tuple type");
            let aggregate = AggregateType::Tuple(tuple.clone());
            format!("(({}){{ ._unit = 0 }})", aggregate_typedef_name(&aggregate))
        }
        Constant::Bool(value) => value.to_string(),
        Constant::Int8(value) => format!("INT8_C({value})"),
        Constant::Int16(value) => format!("INT16_C({value})"),
        Constant::Int32(value) => format!("INT32_C({value})"),
        Constant::Int64(value) => format!("INT64_C({value})"),
        Constant::Uint8(value) => format!("UINT8_C({value})"),
        Constant::Uint16(value) => format!("UINT16_C({value})"),
        Constant::Uint32(value) => format!("UINT32_C({value})"),
        Constant::Uint64(value) => format!("UINT64_C({value})"),
        Constant::Isize(value) => format!("((intptr_t)INT64_C({value}))"),
        Constant::Usize(value) => format!("((uintptr_t)UINT64_C({value}))"),
        Constant::Float32(bits) => {
            let value = f32::from_bits(*bits);
            if value.is_nan() {
                "NAN".to_owned()
            } else if value == f32::INFINITY {
                "INFINITY".to_owned()
            } else if value == f32::NEG_INFINITY {
                "(-INFINITY)".to_owned()
            } else {
                format!("{value:?}f")
            }
        }
        Constant::CInt(value) => value.to_string(),
        Constant::CStr(value) => c_string_literal(value),
        Constant::NullPointer(ty) => format!("(({})0)", type_name(ty)),
    }
}

fn c_string_literal(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7E => output.push(char::from(byte)),
            _ => write!(output, "\\{byte:03o}").unwrap(),
        }
    }
    output.push('\"');
    output
}
