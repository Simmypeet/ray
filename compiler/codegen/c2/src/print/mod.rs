//! Printing of function definitions.
//!
//! Printing is synchronous: it runs after a fragment's dependencies have been
//! collected and resolved, and writes straight into the definitions buffer.
//! Statements are printed here; [`value`] prints operands and rvalues, and
//! [`place`] computes the types of places.

use std::fmt::{self, Write as _};

use rayc_mono_ir::{
    MonoIR,
    cfg::Terminator,
    function::{LocalKind, MonoFunction, MonoFunctionID},
    instruction::{Call, Instruction},
    operand::Operand,
    ty::{MonoType, ReturnType},
};

use crate::{
    aggregates::AggregateRegistry,
    c::{
        expr::PlaceExpr,
        name::{BlockName, FragmentName, FunctionName, LocalName},
        ty::{Declaration, SignatureDeclaration},
    },
    functions::FunctionRegistry,
};

mod place;
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
#[derive(Debug, Clone, Copy)]
pub(crate) struct FunctionPrinter<'a> {
    function: &'a MonoFunction,
    fragment: &'a FragmentFunctions,
    functions: &'a FunctionRegistry,
    aggregates: &'a AggregateRegistry,
}

impl<'a> FunctionPrinter<'a> {
    pub(crate) const fn new(
        function: &'a MonoFunction,
        fragment: &'a FragmentFunctions,
        functions: &'a FunctionRegistry,
        aggregates: &'a AggregateRegistry,
    ) -> Self {
        Self { function, fragment, functions, aggregates }
    }

    /// Writes the definition of the function called `name`.
    pub(crate) fn print(&self, out: &mut String, name: FunctionName) -> fmt::Result {
        let function = self.function;
        let signature = SignatureDeclaration::new(function.signature(), &name);
        writeln!(out, "{} {{", signature.with_parameters_of(function))?;

        // Locals other than parameters are declared up front.
        let mut has_body_locals = false;
        for (local_id, local) in function.locals() {
            if is_declared_in_body(local.kind()) {
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
                self.write_instruction(out, instruction)?;
                out.push('\n');
            }
            let terminator =
                block.terminator().expect("reachable MonoIR block should have a terminator");
            out.push_str("    ");
            self.write_terminator(out, terminator)?;
            out.push('\n');
        }

        out.push('}');
        Ok(())
    }

    fn write_instruction(&self, out: &mut String, instruction: &Instruction) -> fmt::Result {
        match instruction {
            Instruction::Assign(assign) => {
                let destination = assign.destination();
                write!(out, "{} = ", PlaceExpr(destination))?;
                let expected = self.place_type(destination).as_value();
                self.write_rvalue(out, assign.value(), expected)?;
                out.push(';');
                Ok(())
            }
            Instruction::Call(call) => self.write_call(out, call),
        }
    }

    fn write_call(&self, out: &mut String, call: &Call) -> fmt::Result {
        let signature = match call.callee() {
            Operand::Function(callee) => callee.signature(),
            Operand::Copy(place) => self.place_type(place).expect_function_signature(),
            Operand::Constant(_) => panic!("a constant cannot be used as a MonoIR call target"),
        };
        if !signature.is_variadic() {
            assert_eq!(
                call.arguments().len(),
                signature.parameter_types().len(),
                "a non-variadic MonoIR call should match its signature"
            );
        }

        if let Some(destination) = call.destination() {
            write!(out, "{} = ", PlaceExpr(destination))?;
        }
        self.write_operand(out, call.callee(), None)?;
        out.push('(');
        for (index, argument) in call.arguments().iter().enumerate() {
            if index != 0 {
                out.push_str(", ");
            }
            let expected = signature.parameter_types().get(index).map(AsRef::as_ref);
            self.write_operand(out, argument, expected)?;
        }
        out.push_str(");");
        Ok(())
    }

    fn write_terminator(&self, out: &mut String, terminator: &Terminator) -> fmt::Result {
        match terminator {
            Terminator::Goto(target) => write!(out, "goto {};", BlockName(*target)),
            Terminator::Branch(branch) => {
                out.push_str("if (");
                self.write_operand(out, branch.condition(), Some(&MonoType::Bool))?;
                write!(
                    out,
                    ") goto {}; else goto {};",
                    BlockName(branch.then_block()),
                    BlockName(branch.else_block())
                )
            }
            Terminator::Return(Some(value)) => {
                let ReturnType::Value(return_type) = self.function.signature().return_type() else {
                    panic!("a void MonoIR return cannot carry an operand")
                };
                out.push_str("return ");
                self.write_operand(out, value, Some(return_type))?;
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
