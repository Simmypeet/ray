//! Printing of function definitions.
//!
//! The printer writes through a [`CWriter`], which records every aggregate
//! type the printed code spells, and itself records every function the code
//! calls. Statements are printed here; [`value`] prints operands and rvalues.
//!
//! A value of a zero-sized type holds nothing, so it is never stored, passed
//! or returned: a local of such a type is not declared, an assignment to one
//! is left out, and a call passes no argument for one. Every rvalue other than
//! a heap allocation is pure, and none of a zero-sized type allocates, so
//! leaving an assignment out loses nothing.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoFunctionInstance,
    cfg::Terminator,
    function::{LocalKind, MonoFunction},
    instruction::{Call, Instruction},
    operand::{Constant, Operand},
    place::Place,
    ty::{AggregateType, MonoType},
};

use crate::{
    c::{
        expr::PlaceExpr,
        name::{BlockName, LocalName},
        ty::Declaration,
        writer::CWriter,
    },
    projection::projected_type,
};

mod value;

/// Prints the definition of one function.
#[derive(Debug)]
pub(crate) struct FunctionPrinter<'f, 'engine> {
    writer: CWriter<'engine>,
    function: &'f MonoFunction,
    /// Every function the printed code refers to, in the order the references
    /// appear and possibly repeated.
    callees: Vec<MonoFunctionInstance>,
}

impl<'f, 'engine> FunctionPrinter<'f, 'engine> {
    /// A printer of `function` that continues writing into `writer`.
    pub(crate) const fn new(writer: CWriter<'engine>, function: &'f MonoFunction) -> Self {
        Self { writer, function, callees: Vec::new() }
    }

    /// Writes the definition of the function, headed by its `declaration`.
    pub(crate) async fn print(&mut self, declaration: &str) {
        let function = self.function;
        self.writer.push(format_args!("{declaration} {{\n"));

        // locals other than parameters are declared up front, unless they
        // hold nothing
        let mut has_body_locals = false;
        for (local_id, local) in function.locals() {
            if !is_declared_in_body(local.kind()) || self.writer.is_zero_sized(local.ty()).await {
                continue;
            }
            self.writer.push("    ");
            self.writer.write(&Declaration::new(local.ty(), &LocalName(local_id))).await;
            self.writer.push(";\n");
            has_body_locals = true;
        }
        if has_body_locals {
            self.writer.push("\n");
        }

        // blocks follow as labels, starting at the entry block, which C
        // enters first; the rest follow in index order
        let mut blocks = function.blocks().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|(block_id, _)| {
            (*block_id != function.entry_block(), block_id.index())
        });
        for (block_id, block) in blocks {
            self.writer.push(format_args!("{}:\n", BlockName(block_id)));
            for instruction in block.instructions() {
                self.write_instruction(instruction).await;
            }
            let terminator =
                block.terminator().expect("reachable MonoFunction block should have a terminator");
            self.writer.push("    ");
            self.write_terminator(terminator).await;
            self.writer.push("\n");
        }

        self.writer.push("}");
    }

    /// The definition written, the functions it refers to, and the aggregate
    /// types spelled by everything the writer was used for.
    pub(crate) fn finish(self) -> (String, Vec<MonoFunctionInstance>, Vec<AggregateType>) {
        let (definition, aggregates) = self.writer.finish();
        (definition, self.callees, aggregates)
    }

    /// Writes the statement of `instruction` as a line of its own, or nothing
    /// if it only moves a value that holds nothing.
    async fn write_instruction(&mut self, instruction: &Instruction) {
        match instruction {
            Instruction::Assign(assign) => {
                if self.is_place_zero_sized(assign.destination()).await {
                    return;
                }
                self.writer.push(format_args!("    {} = ", PlaceExpr(assign.destination())));
                self.write_rvalue(assign.value()).await;
                self.writer.push(";\n");
            }
            Instruction::Call(call) => self.write_call(call).await,

            // a zero-sized value is never allocated, so it is never freed
            Instruction::HeapFree(pointer) => {
                if self.points_to_zero_sized(pointer).await {
                    return;
                }
                self.writer.push("    ray_free(");
                self.write_operand(pointer).await;
                self.writer.push(");\n");
            }
        }
    }

    /// Writes `destination = callee(arguments);`. The call is always made,
    /// since it may have effects, but a result or an argument that holds
    /// nothing is not passed: the callee neither returns nor takes it.
    async fn write_call(&mut self, call: &Call) {
        self.writer.push("    ");
        if let Some(destination) = call.destination()
            && !self.is_place_zero_sized(destination).await
        {
            self.writer.push(format_args!("{} = ", PlaceExpr(destination)));
        }
        self.write_operand(call.callee()).await;

        self.writer.push("(");
        let mut is_first = true;
        for argument in call.abi_arguments() {
            if self.is_operand_zero_sized(argument).await {
                continue;
            }
            if !is_first {
                self.writer.push(", ");
            }
            is_first = false;
            self.write_operand(argument).await;
        }
        self.writer.push(");\n");
    }

    async fn write_terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.writer.push(format_args!("goto {};", BlockName(*target)));
            }
            Terminator::Branch(branch) => {
                self.writer.push("if (");
                self.write_operand(branch.condition()).await;
                self.writer.push(format_args!(
                    ") goto {}; else goto {};",
                    BlockName(branch.then_block()),
                    BlockName(branch.else_block())
                ));
            }
            Terminator::Switch(switch) => {
                self.writer.push("switch (");
                self.write_operand(switch.discriminant()).await;
                self.writer.push(") {");
                for (value, target) in switch.targets() {
                    self.writer.push(" case ");
                    self.write_operand_constant(value).await;
                    self.writer.push(format_args!(": goto {};", BlockName(target)));
                }
                self.writer
                    .push(format_args!(" default: goto {}; }}", BlockName(switch.otherwise())));
            }
            // a function whose result holds nothing returns `void`
            Terminator::Return(Some(value)) if !self.is_operand_zero_sized(value).await => {
                self.writer.push("return ");
                self.write_operand(value).await;
                self.writer.push(";");
            }
            Terminator::Return(Some(_) | None) => self.writer.push("return;"),
            Terminator::Unreachable => self.writer.push("__builtin_unreachable();"),
        }
    }

    /// The type of `place`: the type of its local, narrowed by each
    /// projection in turn.
    async fn place_type(&self, place: &Place) -> Interned<MonoType> {
        let mut ty = self.function.get_local(place.local()).ty().clone();
        for projection in place.projections() {
            ty = projected_type(self.writer.engine(), &ty, *projection).await;
        }
        ty
    }

    /// Whether `place` holds nothing.
    async fn is_place_zero_sized(&self, place: &Place) -> bool {
        let ty = self.place_type(place).await;
        self.writer.is_zero_sized(&ty).await
    }

    /// Whether the value of `operand` holds nothing.
    async fn is_operand_zero_sized(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Copy(place) => self.is_place_zero_sized(place).await,
            Operand::Constant(constant) => self.is_constant_zero_sized(constant).await,
            // a function is spelled as its address
            Operand::Function(_) => false,
        }
    }

    /// Whether `constant` holds nothing.
    async fn is_constant_zero_sized(&self, constant: &Constant) -> bool {
        match constant {
            Constant::Unit => true,
            Constant::EmptyAggregate(aggregate) => {
                let ty = MonoType::Aggregate(aggregate.clone());
                self.writer.is_zero_sized(&ty).await
            }
            Constant::Bool(_)
            | Constant::Int8(_)
            | Constant::Int16(_)
            | Constant::Int32(_)
            | Constant::Int64(_)
            | Constant::Isize(_)
            | Constant::Uint8(_)
            | Constant::Uint16(_)
            | Constant::Uint32(_)
            | Constant::Uint64(_)
            | Constant::Usize(_)
            | Constant::Float32(_)
            | Constant::CInt(_)
            | Constant::CStr(_)
            | Constant::NullPointer(_) => false,
        }
    }

    /// Whether `pointer`, an operand of a pointer type, points to a value that
    /// holds nothing.
    async fn points_to_zero_sized(&self, pointer: &Operand) -> bool {
        match pointer {
            Operand::Copy(place) => {
                let ty = self.place_type(place).await;
                self.writer.is_zero_sized(ty.assert_as_pointer().pointee()).await
            }
            Operand::Constant(_) | Operand::Function(_) => false,
        }
    }
}

/// Whether a local is declared at the top of the function body rather than
/// in its parameter list.
const fn is_declared_in_body(kind: LocalKind) -> bool {
    match kind {
        LocalKind::Parameter => false,
        LocalKind::Variable | LocalKind::Temporary => true,
    }
}
