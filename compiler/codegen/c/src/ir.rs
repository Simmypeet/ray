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

            for block_id in &layout.blocks {
                writer
                    .write_indent_line(async |writer| {
                        write!(writer, "ray_block_{:X}:", block_id.index())
                    })
                    .await?;

                for instruction in function.block_instructions(*block_id) {
                    writer.write_ir_instruction(instruction, function, &layout, ctx).await?;
                }

                writer
                    .write_ir_terminator(
                        *block_id,
                        function
                            .block_terminator(*block_id)
                            .expect("validated reachable IR block should have a terminator"),
                        function,
                        ctx,
                    )
                    .await?;
            }

            Ok(())
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
        block_id: BlockID,
        terminator: &Terminator,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        match terminator {
            Terminator::Jump(successor) => {
                if Self::block_has_phis(*successor, function) {
                    self.write_indent_line(async |writer| {
                        writer
                            .write_braced_block(async |writer| {
                                writer.write_ir_edge(block_id, *successor, function, ctx).await
                            })
                            .await
                    })
                    .await
                } else {
                    self.write_indent_line(async |writer| {
                        write!(writer, "goto ray_block_{:X};", successor.index())
                    })
                    .await
                }
            }
            Terminator::Conditional(conditional) => {
                self.write_indent_line(async |writer| {
                    write!(writer, "if (ray_expr_{:X}) ", conditional.condition().index())?;
                    writer
                        .write_braced_block(async |writer| {
                            writer
                                .write_ir_edge(block_id, conditional.then_block(), function, ctx)
                                .await
                        })
                        .await?;
                    write!(writer, " else ")?;
                    writer
                        .write_braced_block(async |writer| {
                            writer
                                .write_ir_edge(block_id, conditional.else_block(), function, ctx)
                                .await
                        })
                        .await
                })
                .await
            }
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
        }
    }

    fn block_has_phis(block_id: BlockID, function: &Function) -> bool {
        function.block_instructions(block_id).iter().any(|instruction| {
            let Instruction::Expression(expression_id) = instruction else {
                return false;
            };
            matches!(function.get_expression(*expression_id).kind(), ExpressionKind::Phi(_))
        })
    }

    async fn write_ir_edge(
        &mut self,
        predecessor: BlockID,
        successor: BlockID,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        for instruction in function.block_instructions(successor) {
            let Instruction::Expression(phi_id) = instruction else {
                continue;
            };
            let ExpressionKind::Phi(phi) = function.get_expression(*phi_id).kind() else {
                continue;
            };
            let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                panic!(
                    "invalid IR reached C codegen: phi {} in block {} has no incoming value from \
                     predecessor {}",
                    phi_id.index(),
                    successor.index(),
                    predecessor.index()
                )
            });

            self.write_indent_line(async |writer| {
                let cty = ctx.ty_to_cty(function.get_expression(*phi_id).ty());
                ctx.write_cty(&cty, writer)?;
                write!(
                    writer,
                    " ray_phi_in_{:X}_{:X}_{:X} = ray_expr_{:X};",
                    predecessor.index(),
                    successor.index(),
                    phi_id.index(),
                    incoming.index()
                )
            })
            .await?;
        }

        for instruction in function.block_instructions(successor) {
            let Instruction::Expression(phi_id) = instruction else {
                continue;
            };
            if !matches!(function.get_expression(*phi_id).kind(), ExpressionKind::Phi(_)) {
                continue;
            }

            self.write_indent_line(async |writer| {
                write!(
                    writer,
                    "ray_expr_{:X} = ray_phi_in_{:X}_{:X}_{:X};",
                    phi_id.index(),
                    predecessor.index(),
                    successor.index(),
                    phi_id.index()
                )
            })
            .await?;
        }

        self.write_indent_line(async |writer| {
            write!(writer, "goto ray_block_{:X};", successor.index())
        })
        .await
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
        cfg::{BlockID, Conditional, Terminator},
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
        expression_in(function, function.entry_block(), kind, ty)
    }

    fn expression_in(
        function: &mut Function,
        block_id: BlockID,
        kind: ExpressionKind,
        ty: qbice::storage::intern::Interned<Ty>,
    ) -> ExpressionID {
        let id = function.insert_expression(Expression::new(kind, span(), ty));
        function.push_expression(block_id, id);
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
             bool ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = 1;\n    ray_expr_1 = 2;\n    \
             ray_expr_2 = (ray_expr_0 + ray_expr_1);\n    ray_expr_3 = true;\n    return \
             ray_expr_2;\n}"
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
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    ray_block_0:\n    \
             ray_expr_0 = ray_produce();\n    ray_expr_1 = (ray_expr_0 + ray_expr_0);\n    return \
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
                 ray_expr_1;\n    int32_t ray_expr_2;\n    ray_block_0:\n    ray_expr_0 = 7;\n    \
                 ray_var_0.elem0 = ray_expr_0;\n    ray_expr_1 = ray_var_0.elem0;\n    ray_expr_2 \
                 = ray_param_3.elem1.elemA;\n    return ray_expr_2;\n}}"
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
             ray_block_0:\n    ray_expr_0 = &(ray_var_0);\n    ray_expr_1 = (*ray_expr_0);\n    \
             return ray_expr_1;\n}"
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
                 ray_expr_2;\n    {pair_name} ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = \
                 1;\n    ray_expr_1 = 2;\n    ray_expr_2 = (({unit_name}){{0}});\n    ray_expr_3 \
                 = (({pair_name}){{.elem0 = ray_expr_0,.elem1 = ray_expr_1}});\n    return \
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
            format!("{{\n    ray_block_0:\n    return (({unit_name}){{0}});\n}}")
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
            "{\n    int32_t ray_expr_0;\n    ray_block_0:\n    return ray_expr_0;\n}"
        );
    }

    #[tokio::test]
    async fn diamond_assigns_phi_on_each_incoming_edge() {
        let (engine, mut context) = setup().await;
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let then_block = function.create_block();
        let else_block = function.create_block();
        let merge_block = function.create_block();
        let condition =
            expression(&mut function, ExpressionKind::Literal(Literal::Bool(true)), bool_ty);
        let then_value = expression_in(
            &mut function,
            then_block,
            ExpressionKind::Literal(Literal::Numeric(10)),
            int32.clone(),
        );
        let else_value = expression_in(
            &mut function,
            else_block,
            ExpressionKind::Literal(Literal::Numeric(20)),
            int32.clone(),
        );
        let phi = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(
                [(then_block, then_value), (else_block, else_value)].into_iter().collect(),
            )),
            int32,
        );
        function.set_terminator(
            function.entry_block(),
            Terminator::Conditional(Conditional::new(condition, then_block, else_block)),
        );
        function.set_terminator(then_block, Terminator::Jump(merge_block));
        function.set_terminator(else_block, Terminator::Jump(merge_block));
        function.set_terminator(merge_block, Terminator::Return(Some(phi)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    bool ray_expr_0;\n    int32_t ray_expr_1;\n    int32_t ray_expr_2;\n    \
             int32_t ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = true;\n    if (ray_expr_0) \
             {\n        goto ray_block_1;\n    } else {\n        goto ray_block_2;\n    }\n    \
             ray_block_1:\n    ray_expr_1 = 10;\n    {\n        int32_t ray_phi_in_1_3_3 = \
             ray_expr_1;\n        ray_expr_3 = ray_phi_in_1_3_3;\n        goto ray_block_3;\n    \
             }\n    ray_block_2:\n    ray_expr_2 = 20;\n    {\n        int32_t ray_phi_in_2_3_3 = \
             ray_expr_2;\n        ray_expr_3 = ray_phi_in_2_3_3;\n        goto ray_block_3;\n    \
             }\n    ray_block_3:\n    return ray_expr_3;\n}"
        );
    }

    async fn render_short_circuit(short_circuit_value: bool) -> String {
        let def_id = TargetID::TEST.make_global(SymbolID::default());
        let (engine, mut context) = setup_with_name(def_id, "rhs").await;
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let mut function = Function::new();
        let rhs_block = function.create_block();
        let short_block = function.create_block();
        let merge_block = function.create_block();
        let left = expression(
            &mut function,
            ExpressionKind::Literal(Literal::Bool(!short_circuit_value)),
            bool_ty.clone(),
        );
        let rhs = expression_in(
            &mut function,
            rhs_block,
            ExpressionKind::Call(Call::new(def_id, Vec::new())),
            bool_ty.clone(),
        );
        let short = expression_in(
            &mut function,
            short_block,
            ExpressionKind::Literal(Literal::Bool(short_circuit_value)),
            bool_ty.clone(),
        );
        let result = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(
                [(rhs_block, rhs), (short_block, short)].into_iter().collect(),
            )),
            bool_ty,
        );
        let (then_block, else_block) =
            if short_circuit_value { (short_block, rhs_block) } else { (rhs_block, short_block) };
        function.set_terminator(
            function.entry_block(),
            Terminator::Conditional(Conditional::new(left, then_block, else_block)),
        );
        function.set_terminator(rhs_block, Terminator::Jump(merge_block));
        function.set_terminator(short_block, Terminator::Jump(merge_block));
        function.set_terminator(merge_block, Terminator::Return(Some(result)));

        render(&function, &mut context).await
    }

    #[tokio::test]
    async fn logical_and_keeps_rhs_call_in_the_rhs_block() {
        let body = render_short_circuit(false).await;
        assert_eq!(
            body,
            "{\n    bool ray_expr_0;\n    bool ray_expr_1;\n    bool ray_expr_2;\n    bool \
             ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = true;\n    if (ray_expr_0) {\n        \
             goto ray_block_1;\n    } else {\n        goto ray_block_2;\n    }\n    \
             ray_block_1:\n    ray_expr_1 = ray_rhs();\n    {\n        bool ray_phi_in_1_3_3 = \
             ray_expr_1;\n        ray_expr_3 = ray_phi_in_1_3_3;\n        goto ray_block_3;\n    \
             }\n    ray_block_2:\n    ray_expr_2 = false;\n    {\n        bool ray_phi_in_2_3_3 = \
             ray_expr_2;\n        ray_expr_3 = ray_phi_in_2_3_3;\n        goto ray_block_3;\n    \
             }\n    ray_block_3:\n    return ray_expr_3;\n}"
        );
        assert_eq!(body.matches("ray_rhs()").count(), 1);
    }

    #[tokio::test]
    async fn logical_or_keeps_rhs_call_in_the_rhs_block() {
        let body = render_short_circuit(true).await;
        assert_eq!(
            body,
            "{\n    bool ray_expr_0;\n    bool ray_expr_1;\n    bool ray_expr_2;\n    bool \
             ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = false;\n    if (ray_expr_0) \
             {\n        goto ray_block_2;\n    } else {\n        goto ray_block_1;\n    }\n    \
             ray_block_1:\n    ray_expr_1 = ray_rhs();\n    {\n        bool ray_phi_in_1_3_3 \
             = ray_expr_1;\n        ray_expr_3 = ray_phi_in_1_3_3;\n        goto \
             ray_block_3;\n    }\n    ray_block_2:\n    ray_expr_2 = true;\n    {\n        bool \
             ray_phi_in_2_3_3 = ray_expr_2;\n        ray_expr_3 = \
             ray_phi_in_2_3_3;\n        goto ray_block_3;\n    }\n    ray_block_3:\n    \
             return ray_expr_3;\n}"
        );
        assert_eq!(body.matches("ray_rhs()").count(), 1);
    }

    #[tokio::test]
    async fn same_target_conditional_emits_edge_copies_in_both_arms() {
        let (engine, mut context) = setup().await;
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let entry_block = function.entry_block();
        let merge_block = function.create_block();
        let condition =
            expression(&mut function, ExpressionKind::Literal(Literal::Bool(true)), bool_ty);
        let value =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(7)), int32.clone());
        let phi = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(std::iter::once((entry_block, value)).collect())),
            int32,
        );
        function.set_terminator(
            function.entry_block(),
            Terminator::Conditional(Conditional::new(condition, merge_block, merge_block)),
        );
        function.set_terminator(merge_block, Terminator::Return(Some(phi)));

        let body = render(&function, &mut context).await;
        assert_eq!(
            body,
            "{\n    bool ray_expr_0;\n    int32_t ray_expr_1;\n    int32_t ray_expr_2;\n    \
             ray_block_0:\n    ray_expr_0 = true;\n    ray_expr_1 = 7;\n    if (ray_expr_0) \
             {\n        int32_t ray_phi_in_0_1_2 = ray_expr_1;\n        ray_expr_2 = \
             ray_phi_in_0_1_2;\n        goto ray_block_1;\n    } else {\n        int32_t \
             ray_phi_in_0_1_2 = ray_expr_1;\n        ray_expr_2 = ray_phi_in_0_1_2;\n        \
             goto ray_block_1;\n    }\n    ray_block_1:\n    return ray_expr_2;\n}"
        );
        assert_eq!(body.matches("ray_phi_in_0_1_2 = ray_expr_1").count(), 2);
    }

    #[tokio::test]
    async fn multiple_phis_stage_all_sources_before_assigning_destinations() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let entry_block = function.entry_block();
        let merge_block = function.create_block();
        let first =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let second =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(2)), int32.clone());
        let first_phi = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(std::iter::once((entry_block, first)).collect())),
            int32.clone(),
        );
        let second_phi = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(std::iter::once((entry_block, second)).collect())),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Jump(merge_block));
        function.set_terminator(merge_block, Terminator::Return(Some(second_phi)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    int32_t ray_expr_2;\n    \
             int32_t ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = 1;\n    ray_expr_1 = 2;\n    \
             {\n        int32_t ray_phi_in_0_1_2 = ray_expr_0;\n        int32_t ray_phi_in_0_1_3 \
             = ray_expr_1;\n        ray_expr_2 = ray_phi_in_0_1_2;\n        ray_expr_3 = \
             ray_phi_in_0_1_3;\n        goto ray_block_1;\n    }\n    ray_block_1:\n    return \
             ray_expr_3;\n}"
        );

        let _ = first_phi;
    }

    #[tokio::test]
    async fn loop_carries_phi_value_across_the_back_edge() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let mut function = Function::new();
        let entry_block = function.entry_block();
        let header_block = function.create_block();
        let body_block = function.create_block();
        let exit_block = function.create_block();
        let initial =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let next_id = ExpressionID::new(3);
        let carried = expression_in(
            &mut function,
            header_block,
            ExpressionKind::Phi(Phi::new(
                [(entry_block, initial), (body_block, next_id)].into_iter().collect(),
            )),
            int32.clone(),
        );
        let condition = expression_in(
            &mut function,
            header_block,
            ExpressionKind::Literal(Literal::Bool(false)),
            bool_ty,
        );
        let next = expression_in(
            &mut function,
            body_block,
            ExpressionKind::Binary(Binary::new(carried, BinaryOp::Plus, initial)),
            int32,
        );
        assert_eq!(next, next_id);
        function.set_terminator(entry_block, Terminator::Jump(header_block));
        function.set_terminator(
            header_block,
            Terminator::Conditional(Conditional::new(condition, body_block, exit_block)),
        );
        function.set_terminator(body_block, Terminator::Jump(header_block));
        function.set_terminator(exit_block, Terminator::Return(Some(carried)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    bool ray_expr_2;\n    \
             int32_t ray_expr_3;\n    ray_block_0:\n    ray_expr_0 = 1;\n    {\n        int32_t \
             ray_phi_in_0_1_1 = ray_expr_0;\n        ray_expr_1 = ray_phi_in_0_1_1;\n        goto \
             ray_block_1;\n    }\n    ray_block_1:\n    ray_expr_2 = false;\n    if (ray_expr_2) \
             {\n        goto ray_block_2;\n    } else {\n        goto ray_block_3;\n    }\n    \
             ray_block_2:\n    ray_expr_3 = (ray_expr_1 + ray_expr_0);\n    {\n        int32_t \
             ray_phi_in_2_1_1 = ray_expr_3;\n        ray_expr_1 = ray_phi_in_2_1_1;\n        goto \
             ray_block_1;\n    }\n    ray_block_3:\n    return ray_expr_1;\n}"
        );
    }

    #[tokio::test]
    async fn loop_carried_swap_stages_both_sources_before_either_destination() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let mut function = Function::new();
        let entry_block = function.entry_block();
        let header_block = function.create_block();
        let body_block = function.create_block();
        let exit_block = function.create_block();
        let initial_a =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let initial_b =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(2)), int32.clone());
        let phi_b_id = ExpressionID::new(3);
        let phi_a = expression_in(
            &mut function,
            header_block,
            ExpressionKind::Phi(Phi::new(
                [(entry_block, initial_a), (body_block, phi_b_id)].into_iter().collect(),
            )),
            int32.clone(),
        );
        let phi_b = expression_in(
            &mut function,
            header_block,
            ExpressionKind::Phi(Phi::new(
                [(entry_block, initial_b), (body_block, phi_a)].into_iter().collect(),
            )),
            int32,
        );
        assert_eq!(phi_b, phi_b_id);
        let condition = expression_in(
            &mut function,
            header_block,
            ExpressionKind::Literal(Literal::Bool(false)),
            bool_ty,
        );
        function.set_terminator(entry_block, Terminator::Jump(header_block));
        function.set_terminator(
            header_block,
            Terminator::Conditional(Conditional::new(condition, body_block, exit_block)),
        );
        function.set_terminator(body_block, Terminator::Jump(header_block));
        function.set_terminator(exit_block, Terminator::Return(Some(phi_a)));

        assert_eq!(
            render(&function, &mut context).await,
            "{\n    int32_t ray_expr_0;\n    int32_t ray_expr_1;\n    int32_t ray_expr_2;\n    \
             int32_t ray_expr_3;\n    bool ray_expr_4;\n    ray_block_0:\n    ray_expr_0 = \
             1;\n    ray_expr_1 = 2;\n    {\n        int32_t ray_phi_in_0_1_2 = \
             ray_expr_0;\n        int32_t ray_phi_in_0_1_3 = ray_expr_1;\n        \
             ray_expr_2 = ray_phi_in_0_1_2;\n        ray_expr_3 = ray_phi_in_0_1_3;\n        \
             goto ray_block_1;\n    }\n    ray_block_1:\n    ray_expr_4 = false;\n    if \
             (ray_expr_4) {\n        goto ray_block_2;\n    } else {\n        goto \
             ray_block_3;\n    }\n    ray_block_2:\n    {\n        int32_t ray_phi_in_2_1_2 = \
             ray_expr_3;\n        int32_t ray_phi_in_2_1_3 = ray_expr_2;\n        \
             ray_expr_2 = ray_phi_in_2_1_2;\n        ray_expr_3 = ray_phi_in_2_1_3;\n        \
             goto ray_block_1;\n    }\n    ray_block_3:\n    return ray_expr_2;\n}"
        );
    }

    #[tokio::test]
    #[should_panic(expected = "invalid IR reached C codegen: phi 1 in block 1 has no incoming \
                               value from predecessor 0")]
    async fn phi_missing_predecessor_input_panics() {
        let (engine, mut context) = setup().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut function = Function::new();
        let merge_block = function.create_block();
        let _value =
            expression(&mut function, ExpressionKind::Literal(Literal::Numeric(1)), int32.clone());
        let phi = expression_in(
            &mut function,
            merge_block,
            ExpressionKind::Phi(Phi::new(rayc_hash::FxHashMap::default())),
            int32,
        );
        function.set_terminator(function.entry_block(), Terminator::Jump(merge_block));
        function.set_terminator(merge_block, Terminator::Return(Some(phi)));

        let _ = render(&function, &mut context).await;
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
