//! Printing of function definitions.
//!
//! The printer writes straight into a definition buffer and records, through
//! [`Program`], everything the printed code refers to: the aggregate types it
//! spells and the fragments it calls. Every type is recorded where it is
//! printed, so a type can only appear in the output if its definition will
//! too. Statements are printed here; [`value`] prints operands and rvalues.

use std::fmt::{self, Write as _};

use rayc_mono_ir::{
    MonoIR,
    cfg::Terminator,
    function::{LocalKind, MonoFunction, MonoFunctionID},
    instruction::{Call, Instruction},
};

use crate::{
    c::{
        expr::PlaceExpr,
        name::{BlockName, FragmentName, FunctionName, LocalName},
        ty::{Declaration, FunctionDeclaration},
    },
    program::Program,
};

mod value;

/// The functions of one fragment with their C names, in ID order.
#[derive(Debug)]
pub(crate) struct FragmentFunctions {
    entries: Vec<(MonoFunctionID, FunctionName)>,
}

impl FragmentFunctions {
    pub(crate) fn of(ir: &MonoIR) -> Self {
        let fragment = FragmentName::of(ir.instance());
        let mut entries = ir
            .functions()
            .map(|(id, _)| (id, FunctionName::of(ir, fragment, id)))
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(id, _)| *id);
        Self { entries }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (MonoFunctionID, FunctionName)> + '_ {
        self.entries.iter().copied()
    }

    fn name(&self, function_id: MonoFunctionID) -> FunctionName {
        let index = self
            .entries
            .binary_search_by_key(&function_id, |(id, _)| *id)
            .expect("local function reference should name a function of its fragment");
        self.entries[index].1
    }
}

/// Prints one function definition of a fragment.
#[derive(Debug)]
pub(crate) struct FunctionPrinter<'p, 'engine> {
    program: &'p mut Program<'engine>,
    fragment: &'p FragmentFunctions,
    function: &'p MonoFunction,
}

impl<'p, 'engine> FunctionPrinter<'p, 'engine> {
    pub(crate) const fn new(
        program: &'p mut Program<'engine>,
        fragment: &'p FragmentFunctions,
        function: &'p MonoFunction,
    ) -> Self {
        Self { program, fragment, function }
    }

    /// Writes the definition of the function called `name`.
    pub(crate) async fn print(&mut self, out: &mut String, name: FunctionName) -> fmt::Result {
        let function = self.function;
        self.program.use_signature(function.signature()).await;
        let declaration = FunctionDeclaration::new(function.signature(), &name);
        writeln!(out, "{} {{", declaration.with_parameters_of(function))?;

        // Locals other than parameters are declared up front.
        let mut has_body_locals = false;
        for (local_id, local) in function.locals() {
            if is_declared_in_body(local.kind()) {
                self.program.use_type(local.ty()).await;
                writeln!(out, "    {};", Declaration::new(local.ty(), &LocalName(local_id)))?;
                has_body_locals = true;
            }
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
                out.push_str("    ");
                self.write_instruction(out, instruction).await?;
                out.push('\n');
            }
            let terminator =
                block.terminator().expect("reachable MonoIR block should have a terminator");
            out.push_str("    ");
            self.write_terminator(out, terminator).await?;
            out.push('\n');
        }

        out.push('}');
        Ok(())
    }

    async fn write_instruction(
        &mut self,
        out: &mut String,
        instruction: &Instruction,
    ) -> fmt::Result {
        match instruction {
            Instruction::Assign(assign) => {
                write!(out, "{} = ", PlaceExpr(assign.destination()))?;
                self.write_rvalue(out, assign.value()).await?;
                out.push(';');
                Ok(())
            }
            Instruction::Call(call) => self.write_call(out, call).await,
        }
    }

    async fn write_call(&mut self, out: &mut String, call: &Call) -> fmt::Result {
        if let Some(destination) = call.destination() {
            write!(out, "{} = ", PlaceExpr(destination))?;
        }
        self.write_operand(out, call.callee()).await?;
        out.push('(');
        for (index, argument) in call.arguments().iter().enumerate() {
            if index != 0 {
                out.push_str(", ");
            }
            self.write_operand(out, argument).await?;
        }
        out.push_str(");");
        Ok(())
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
            Terminator::Return(Some(value)) => {
                out.push_str("return ");
                self.write_operand(out, value).await?;
                out.push(';');
                Ok(())
            }
            Terminator::Return(None) => out.write_str("return;"),
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
