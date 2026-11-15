//! Printing of function definitions.
//!
//! The printer writes straight into a definition buffer and records, through
//! [`Program`], everything the printed code refers to: the aggregate types it
//! spells and the functions it calls. Every type is recorded where it is
//! printed, so a type can only appear in the output if its definition will
//! too. Statements are printed here; [`value`] prints operands and rvalues.
//!
//! A value of a zero-sized type holds nothing, so it is never stored, passed
//! or returned: a local of such a type is not declared, an assignment to one
//! is left out, and a call passes no argument for one. Every rvalue other than
//! a heap allocation is pure, and none of a zero-sized type allocates, so
//! leaving an assignment out loses nothing.

use std::fmt::{self, Write as _};

use rayc_mono_ir::{
    cfg::Terminator,
    function::{LocalKind, MonoFunction},
    instruction::{Call, Instruction},
    operand::Operand,
    place::Place,
};

use crate::{
    c::{
        expr::PlaceExpr,
        name::{BlockName, FunctionName, LocalName},
        ty::{Declaration, FunctionDeclaration},
    },
    program::Program,
};

mod value;

/// Prints one function definition.
#[derive(Debug)]
pub(crate) struct FunctionPrinter<'p, 'engine> {
    program: &'p mut Program<'engine>,
    function: &'p MonoFunction,
}

impl<'p, 'engine> FunctionPrinter<'p, 'engine> {
    pub(crate) const fn new(program: &'p mut Program<'engine>, function: &'p MonoFunction) -> Self {
        Self { program, function }
    }

    /// Writes the definition of the function called `name`.
    pub(crate) async fn print(&mut self, out: &mut String, name: FunctionName) -> fmt::Result {
        let function = self.function;
        self.program.use_signature(function.signature()).await;
        let declaration =
            FunctionDeclaration::new(function.signature(), &name, self.program.layouts());
        writeln!(out, "{} {{", declaration.with_parameters_of(function))?;

        // Locals other than parameters are declared up front, unless they
        // hold nothing.
        let mut has_body_locals = false;
        for (local_id, local) in function.locals() {
            if !is_declared_in_body(local.kind()) || self.program.is_zero_sized(local.ty()).await {
                continue;
            }
            let name = LocalName(local_id);
            writeln!(out, "    {};", Declaration::new(local.ty(), &name, self.program.layouts()))?;
            has_body_locals = true;
        }
        if has_body_locals {
            out.push('\n');
        }

        // Blocks follow as labels, starting at the entry block.
        let mut blocks = function.blocks().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|(block_id, _)| {
            (*block_id != function.entry_block(), block_id.index())
        });
        for (block_id, block) in blocks {
            writeln!(out, "{}:", BlockName(block_id))?;
            for instruction in block.instructions() {
                self.write_instruction(out, instruction).await?;
            }
            let terminator =
                block.terminator().expect("reachable MonoFunction block should have a terminator");
            out.push_str("    ");
            self.write_terminator(out, terminator).await?;
            out.push('\n');
        }

        out.push('}');
        Ok(())
    }

    /// Writes the statement of `instruction` as a line of its own, or nothing
    /// if it only moves a value that holds nothing.
    async fn write_instruction(
        &mut self,
        out: &mut String,
        instruction: &Instruction,
    ) -> fmt::Result {
        match instruction {
            Instruction::Assign(assign) => {
                if self.is_place_zero_sized(assign.destination()).await {
                    return Ok(());
                }
                write!(out, "    {} = ", PlaceExpr(assign.destination()))?;
                self.write_rvalue(out, assign.value()).await?;
                out.push_str(";\n");
                Ok(())
            }
            Instruction::Call(call) => self.write_call(out, call).await,

            // A zero-sized value is never allocated, so it is never freed.
            Instruction::HeapFree(pointer) => {
                if self.points_to_zero_sized(pointer).await {
                    return Ok(());
                }
                out.push_str("    ray_free(");
                self.write_operand(out, pointer).await?;
                out.push_str(");\n");
                Ok(())
            }
        }
    }

    /// Writes `destination = callee(arguments);`. The call is always made,
    /// since it may have effects, but a result or an argument that holds
    /// nothing is not passed: the callee neither returns nor takes it.
    async fn write_call(&mut self, out: &mut String, call: &Call) -> fmt::Result {
        out.push_str("    ");
        if let Some(destination) = call.destination()
            && !self.is_place_zero_sized(destination).await
        {
            write!(out, "{} = ", PlaceExpr(destination))?;
        }
        self.write_operand(out, call.callee()).await?;

        out.push('(');
        let mut is_first = true;
        for argument in call.abi_arguments() {
            if self.is_operand_zero_sized(argument).await {
                continue;
            }
            if !is_first {
                out.push_str(", ");
            }
            is_first = false;
            self.write_operand(out, argument).await?;
        }
        out.push_str(");\n");
        Ok(())
    }

    /// Whether `place` holds nothing.
    async fn is_place_zero_sized(&mut self, place: &Place) -> bool {
        self.program.is_place_zero_sized(self.function, place).await
    }

    /// Whether the value of `operand` holds nothing.
    async fn is_operand_zero_sized(&mut self, operand: &Operand) -> bool {
        self.program.is_operand_zero_sized(self.function, operand).await
    }

    /// Whether `pointer`, an operand of a pointer type, points to a value that
    /// holds nothing.
    async fn points_to_zero_sized(&mut self, pointer: &Operand) -> bool {
        match pointer {
            Operand::Copy(place) => {
                let ty = self.program.place_type(self.function, place);
                self.program.is_zero_sized(ty.assert_as_pointer().pointee()).await
            }
            Operand::Constant(_) | Operand::Function(_) => false,
        }
    }

    async fn write_terminator(&mut self, out: &mut String, terminator: &Terminator) -> fmt::Result {
        match terminator {
            Terminator::Goto(target) => write!(out, "goto {};", BlockName(*target)),
            Terminator::Branch(branch) => {
                out.push_str("if (");
                self.write_operand(out, branch.condition()).await?;
                write!(
                    out,
                    ") goto {}; else goto {};",
                    BlockName(branch.then_block()),
                    BlockName(branch.else_block())
                )
            }
            Terminator::Switch(switch) => {
                out.push_str("switch (");
                self.write_operand(out, switch.discriminant()).await?;
                out.push_str(") {");
                for (value, target) in switch.targets() {
                    out.push_str(" case ");
                    self.write_operand_constant(out, value).await?;
                    write!(out, ": goto {};", BlockName(target))?;
                }
                write!(out, " default: goto {}; }}", BlockName(switch.otherwise()))
            }
            // A function whose result holds nothing returns `void`.
            Terminator::Return(Some(value)) if !self.is_operand_zero_sized(value).await => {
                out.push_str("return ");
                self.write_operand(out, value).await?;
                out.push(';');
                Ok(())
            }
            Terminator::Return(Some(_) | None) => out.write_str("return;"),
            Terminator::Unreachable => out.write_str("__builtin_unreachable();"),
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
