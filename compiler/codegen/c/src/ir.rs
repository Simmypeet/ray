use std::io::Write;

use rayc_hash::FxHashSet;
use rayc_ir::{
    address::{Address, AddressRoot, Projection},
    cfg::{BlockID, Instruction, Terminator},
    expression::{ExpressionID, ExpressionKind, binary::BinaryOp, literal::Literal},
    function::Function,
    variable::VariableID,
};

use crate::{context::Context, writer::Writer};

#[derive(Debug)]
struct FunctionLayout {
    blocks: Vec<BlockID>,
    expressions: Vec<ExpressionID>,
    variables: Vec<VariableID>,
    phis: FxHashSet<ExpressionID>,
}

impl FunctionLayout {
    fn new(function: &Function) -> Self {
        function.validate().unwrap_or_else(|error| {
            panic!("invalid IR reached C codegen: {error:?}");
        });

        let mut pending = vec![function.entry_block()];
        let mut reachable_blocks = FxHashSet::default();

        while let Some(block_id) = pending.pop() {
            if !reachable_blocks.insert(block_id) {
                continue;
            }

            match function
                .block_terminator(block_id)
                .expect("validated reachable IR block should have a terminator")
            {
                Terminator::Jump(successor) => pending.push(*successor),
                Terminator::Conditional(conditional) => {
                    pending.push(conditional.then_block());
                    pending.push(conditional.else_block());
                }
                Terminator::Return(_) => {}
            }
        }

        let mut blocks: Vec<_> = reachable_blocks.into_iter().collect();
        blocks.sort_unstable_by_key(BlockID::index);

        let mut reachable_expressions = FxHashSet::default();
        let mut phis = FxHashSet::default();
        for block_id in &blocks {
            for instruction in function.block_instructions(*block_id) {
                let Instruction::Expression(expression_id) = instruction else {
                    continue;
                };

                if reachable_expressions.insert(*expression_id)
                    && matches!(
                        function.get_expression(*expression_id).kind(),
                        ExpressionKind::Phi(_)
                    )
                {
                    phis.insert(*expression_id);
                }
            }
        }

        let mut expressions: Vec<_> = reachable_expressions.into_iter().collect();
        expressions.sort_unstable_by_key(ExpressionID::index);

        let mut variables: Vec<_> = function.variables().map(|(id, _)| id).collect();
        variables.sort_unstable_by_key(VariableID::index);

        Self { blocks, expressions, variables, phis }
    }

    fn is_phi(&self, expression_id: ExpressionID) -> bool { self.phis.contains(&expression_id) }
}

impl Writer<'_> {
    pub(crate) async fn write_ir_function_body(
        &mut self,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let layout = FunctionLayout::new(function);

        assert_eq!(
            layout.blocks.len(),
            1,
            "control-flow IR reached straight-line C codegen before CFG emission was implemented"
        );

        self.write_braced_block(async |writer| {
            for variable_id in &layout.variables {
                writer
                    .write_indent_line(async |writer| {
                        let cty = ctx.ty_to_cty(function.get_variable(*variable_id).ty());
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " ray_var_{:X};", variable_id.index())
                    })
                    .await?;
            }

            for expression_id in &layout.expressions {
                writer
                    .write_indent_line(async |writer| {
                        let cty = ctx.ty_to_cty(function.get_expression(*expression_id).ty());
                        ctx.write_cty(&cty, writer)?;
                        write!(writer, " ray_expr_{:X};", expression_id.index())
                    })
                    .await?;
            }

            let block_id = layout.blocks[0];
            for instruction in function.block_instructions(block_id) {
                writer.write_ir_instruction(instruction, function, &layout, ctx).await?;
            }

            writer
                .write_ir_terminator(
                    function
                        .block_terminator(block_id)
                        .expect("validated reachable IR block should have a terminator"),
                    ctx,
                )
                .await
        })
        .await
    }

    async fn write_ir_instruction(
        &mut self,
        instruction: &Instruction,
        function: &Function,
        layout: &FunctionLayout,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        match instruction {
            Instruction::Expression(expression_id) => {
                if layout.is_phi(*expression_id) {
                    return Ok(());
                }

                self.write_indent_line(async |writer| {
                    write!(writer, "ray_expr_{:X} = ", expression_id.index())?;
                    writer.write_expression_value(*expression_id, function, ctx).await?;
                    write!(writer, ";")
                })
                .await
            }
            Instruction::Store(store) => {
                self.write_indent_line(async |writer| {
                    writer.write_address(store.address())?;
                    write!(writer, " = ray_expr_{:X};", store.expression().index())
                })
                .await
            }
        }
    }

    async fn write_ir_terminator(
        &mut self,
        terminator: &Terminator,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        match terminator {
            Terminator::Return(value) => {
                self.write_indent_line(async |writer| {
                    write!(writer, "return ")?;
                    if let Some(expression_id) = value {
                        write!(writer, "ray_expr_{:X}", expression_id.index())?;
                    } else {
                        writer.generate_unit_value(ctx).await?;
                    }
                    write!(writer, ";")
                })
                .await
            }
            Terminator::Jump(_) | Terminator::Conditional(_) => {
                panic!("control-flow terminator reached straight-line C codegen")
            }
        }
    }

    async fn write_expression_value(
        &mut self,
        expression_id: ExpressionID,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let expression = function.get_expression(expression_id);
        match expression.kind() {
            ExpressionKind::Error => {
                panic!("error expression reached codegen, this should have been caught earlier")
            }
            ExpressionKind::Literal(literal) => match literal {
                Literal::Numeric(value) => write!(self, "{value}"),
                Literal::Bool(true) => write!(self, "true"),
                Literal::Bool(false) => write!(self, "false"),
            },
            ExpressionKind::RefOf(ref_of) => {
                write!(self, "&(")?;
                self.write_address(ref_of.address())?;
                write!(self, ")")
            }
            ExpressionKind::Load(load) => self.write_address(load.address()),
            ExpressionKind::Phi(_) => {
                panic!("phi expression cannot be emitted as an ordinary C expression")
            }
            ExpressionKind::Binary(binary) => {
                let operator = match binary.operator() {
                    BinaryOp::Plus => "+",
                    BinaryOp::Minus => "-",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                };
                write!(
                    self,
                    "(ray_expr_{:X} {operator} ray_expr_{:X})",
                    binary.left().index(),
                    binary.right().index()
                )
            }
            ExpressionKind::Call(call) => {
                let name = ctx.get_def_name(call.function_id()).await;
                write!(self, "ray_{}(", &*name)?;
                for (index, argument) in call.arguments().iter().enumerate() {
                    if index != 0 {
                        write!(self, ", ")?;
                    }
                    write!(self, "ray_expr_{:X}", argument.index())?;
                }
                write!(self, ")")
            }
            ExpressionKind::Tuple(tuple) => {
                write!(self, "((")?;
                let tuple_id = ctx.unwrap_ty_as_ctuple_id(expression.ty());
                ctx.write_ctuple_t(tuple_id, self)?;
                write!(self, "){{")?;

                if tuple.elements().is_empty() {
                    write!(self, "0")?;
                } else {
                    for (index, element) in tuple.elements().iter().enumerate() {
                        if index != 0 {
                            write!(self, ",")?;
                        }
                        write!(self, ".elem{index:X} = ray_expr_{:X}", element.index())?;
                    }
                }

                write!(self, "}})")
            }
        }
    }

    fn write_address(&mut self, address: &Address) -> std::io::Result<()> {
        match address.root() {
            AddressRoot::Error => {
                panic!("error address reached codegen, this should have been caught earlier")
            }
            AddressRoot::Variable(variable_id) => {
                write!(self, "ray_var_{:X}", variable_id.index())?;
            }
            AddressRoot::Parameter(parameter_id) => {
                write!(self, "ray_param_{:X}", parameter_id.index())?;
            }
            AddressRoot::Deref(expression_id) => {
                write!(self, "(*ray_expr_{:X})", expression_id.index())?;
            }
        }

        for projection in address.projections() {
            match projection {
                Projection::Tuple(index) => write!(self, ".elem{index:X}")?,
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::{collections::HashMap, sync::Arc};

    use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
    use rayc_arena::ID;
    use rayc_ir::{
        address::Address,
        cfg::Terminator,
        expression::{
            Expression, ExpressionID, ExpressionKind,
            binary::{Binary, BinaryOp},
            call::Call,
            literal::Literal,
            load::Load,
            phi::Phi,
            ref_of::RefOf,
            tuple::Tuple,
        },
        function::Function,
        variable::Variable,
    };
    use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
    use rayc_semantic_element::parameter::ParameterID;
    use rayc_source_file::LocalSourceID;
    use rayc_symbol::{SymbolID, name};
    use rayc_target::TargetID;
    use rayc_type::ty::{Mutability, Primitive, Ty};

    use super::*;

    fn span() -> RelativeSpan {
        let location = RelativeLocation {
            offset: 0,
            mode: OffsetMode::Start,
            relative_to: ID::<Branch>::new(0),
        };
        RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
    }

    async fn setup() -> (TrackedEngine, Context) {
        let engine = rayc_qbice::create_minimal_engine().await;
        let context = Context::new(engine.clone());
        (engine, context)
    }

    async fn setup_with_name(
        def_id: rayc_symbol::GlobalSymbolID,
        value: &str,
    ) -> (TrackedEngine, Context) {
        let interner = rayc_qbice::create_minimal_engine().await;
        let value = interner.intern_unsized(value.to_owned());
        let mut engine =
            Engine::new_with(Plugin::default(), InMemoryFactory, SeededStableHasherBuilder::new(0))
                .await
                .unwrap();
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            name::Key { symbol_id: def_id },
            value,
        )]))));
        let engine = Arc::new(engine).tracked().await;
        let context = Context::new(engine.clone());
        (engine, context)
    }

    fn expression(
        function: &mut Function,
        kind: ExpressionKind,
        ty: qbice::storage::intern::Interned<Ty>,
    ) -> ExpressionID {
        let id = function.insert_expression(Expression::new(kind, span(), ty));
        function.push_expression(function.entry_block(), id);
        id
    }

    async fn render(function: &Function, context: &mut Context) -> String {
        let mut bytes = Vec::new();
        Writer::new(&mut bytes).write_ir_function_body(function, context).await.unwrap();
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn literals_and_arithmetic_use_declared_temporaries() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let mut function = Function::new();
        let one =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let two =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(2)), int32.clone());
        let sum = expression(
            &mut function,
            ExpressionKind::Binary(Binary::new(one, BinaryOp::Plus, two)),
            int32,
        );
        let boolean =
            expression(&mut function, ExpressionKind::Literal(Literal::Bool(true)), bool_ty);
        function.set_terminator(function.entry_block(), Terminator::Return(Some(sum)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    int32_t ray_expr_2;\n    \
             bool ray_expr_3;\n    ray_expr_0 = 1;\n    ray_expr_1 = 2;\n    ray_expr_2 = \
             (ray_expr_0 + ray_expr_1);\n    ray_expr_3 = true;\n    return ray_expr_2;\n}"
        );

        let _ = boolean;
    }

    #[tokio::test]
    async fn call_with_multiple_consumers_is_emitted_once() {
        let def_id = TargetID::TEST.make_global(SymbolID::default());
        let (engine, mut context) = setup_with_name(def_id, "produce").await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let call = expression(
            &mut function,
            ExpressionKind::Call(Call::new(def_id, Vec::new())),
            int32.clone(),
        );
        let sum = expression(
            &mut function,
            ExpressionKind::Binary(Binary::new(call, BinaryOp::Plus, call)),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Return(Some(sum)));

        let body = render(&function, &mut context).await;
        assert_eq!(
            body,
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    ray_expr_0 = \
             ray_produce();\n    ray_expr_1 = (ray_expr_0 + ray_expr_0);\n    return \
             ray_expr_1;\n}"
        );
        assert_eq!(body.matches("ray_produce()").count(), 1);
    }

    #[tokio::test]
    async fn loads_stores_and_projected_addresses_preserve_instruction_order() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let tuple_ty = Ty::new_tuple(engine.intern_unsized([int32.clone()]), &engine);
        let mut function = Function::new();
        let variable = function.insert_variable(Variable::new(tuple_ty, span()));
        let value =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(7)), int32.clone());
        let mut variable_element = Address::new_variable(variable, &engine);
        variable_element.add_tuple_index(0, &engine);
        function.push_store(function.entry_block(), variable_element.clone(), value);
        let loaded_variable = expression(
            &mut function,
            ExpressionKind::Load(Load::new(variable_element)),
            int32.clone(),
        );
        let mut parameter_element = Address::new_parameter(ParameterID::new(3), &engine);
        parameter_element.add_tuple_index(1, &engine);
        parameter_element.add_tuple_index(10, &engine);
        let loaded_parameter =
            expression(&mut function, ExpressionKind::Load(Load::new(parameter_element)), int32);
        function.set_terminator(function.entry_block(), Terminator::Return(Some(loaded_parameter)));

        let mut tuple_name = Vec::new();
        let tuple_id = context.unwrap_ty_as_ctuple_id(function.get_variable(variable).ty());
        context.write_ctuple_t(tuple_id, &mut tuple_name).unwrap();
        let tuple_name = String::from_utf8(tuple_name).unwrap();
        assert_eq!(
            render(&function, &mut context).await,
            format!(
                "{{\n    {tuple_name} ray_var_0;\n    int32_t ray_expr_0;\n    int32_t \
                 ray_expr_1;\n    int32_t ray_expr_2;\n    ray_expr_0 = 7;\n    ray_var_0.elem0 = \
                 ray_expr_0;\n    ray_expr_1 = ray_var_0.elem0;\n    ray_expr_2 = \
                 ray_param_3.elem1.elemA;\n    return ray_expr_2;\n}}"
            )
        );

        let _ = loaded_variable;
    }

    #[tokio::test]
    async fn reference_and_dereference_addresses_are_rendered_as_lvalues() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let pointer = Ty::new_pointer(int32.clone(), Mutability::Mutable, &engine);
        let mut function = Function::new();
        let variable = function.insert_variable(Variable::new(int32.clone(), span()));
        let reference = expression(
            &mut function,
            ExpressionKind::RefOf(RefOf::new(Address::new_variable(variable, &engine))),
            pointer,
        );
        let dereference = expression(
            &mut function,
            ExpressionKind::Load(Load::new(Address::new_deref(reference, &engine))),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Return(Some(dereference)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_var_0;\n    int32_t* ray_expr_0;\n    int32_t ray_expr_1;\n    \
             ray_expr_0 = &(ray_var_0);\n    ray_expr_1 = (*ray_expr_0);\n    return \
             ray_expr_1;\n}"
        );
    }

    #[tokio::test]
    async fn empty_and_non_empty_tuples_use_the_existing_unit_strategy() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let unit_ty = Ty::new_tuple(engine.intern_unsized([]), &engine);
        let pair_ty = Ty::new_tuple(engine.intern_unsized([int32.clone(), int32.clone()]), &engine);
        let mut function = Function::new();
        let one =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let two = expression(&mut function, ExpressionKind::Literal(Literal::Numeric(2)), int32);
        let unit = expression(
            &mut function,
            ExpressionKind::Tuple(Tuple::new(Vec::new())),
            unit_ty.clone(),
        );
        let pair = expression(
            &mut function,
            ExpressionKind::Tuple(Tuple::new(vec![one, two])),
            pair_ty.clone(),
        );
        function.set_terminator(function.entry_block(), Terminator::Return(Some(pair)));

        let unit_id = context.unwrap_ty_as_ctuple_id(&unit_ty);
        let pair_id = context.unwrap_ty_as_ctuple_id(&pair_ty);
        let mut unit_name = Vec::new();
        let mut pair_name = Vec::new();
        context.write_ctuple_t(unit_id, &mut unit_name).unwrap();
        context.write_ctuple_t(pair_id, &mut pair_name).unwrap();
        let unit_name = String::from_utf8(unit_name).unwrap();
        let pair_name = String::from_utf8(pair_name).unwrap();
        assert_eq!(
            render(&function, &mut context).await,
            format!(
                "{{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    {unit_name} \
                 ray_expr_2;\n    {pair_name} ray_expr_3;\n    ray_expr_0 = 1;\n    ray_expr_1 = \
                 2;\n    ray_expr_2 = (({unit_name}){{0}});\n    ray_expr_3 = \
                 (({pair_name}){{.elem0 = ray_expr_0,.elem1 = ray_expr_1}});\n    return \
                 ray_expr_3;\n}}"
            )
        );

        let _ = unit;
    }

    #[tokio::test]
    async fn bare_return_generates_a_unit_value() {
        let (engine, mut context) = setup().await;
        let mut function = Function::new();
        function.set_terminator(function.entry_block(), Terminator::Return(None));

        let unit_ty = Ty::new_tuple(engine.intern_unsized([]), &engine);
        let unit_id = context.unwrap_ty_as_ctuple_id(&unit_ty);
        let mut unit_name = Vec::new();
        context.write_ctuple_t(unit_id, &mut unit_name).unwrap();
        let unit_name = String::from_utf8(unit_name).unwrap();
        assert_eq!(
            render(&function, &mut context).await,
            format!("{{\n    return (({unit_name}){{0}});\n}}")
        );
    }

    #[tokio::test]
    async fn phi_is_declared_without_an_ordinary_assignment() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let phi = expression(
            &mut function,
            ExpressionKind::Phi(Phi::new(rayc_hash::FxHashMap::default())),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Return(Some(phi)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_expr_0;\n    return ray_expr_0;\n}"
        );
    }

    #[tokio::test]
    #[should_panic(expected = "error expression reached codegen")]
    async fn error_expression_panics() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let error = expression(&mut function, ExpressionKind::Error, int32);
        function.set_terminator(function.entry_block(), Terminator::Return(Some(error)));

        let _ = render(&function, &mut context).await;
    }

    #[tokio::test]
    #[should_panic(expected = "error address reached codegen")]
    async fn error_address_panics() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let error = expression(
            &mut function,
            ExpressionKind::Load(Load::new(Address::new_error(&engine))),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Return(Some(error)));

        let _ = render(&function, &mut context).await;
    }
}
