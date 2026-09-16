Hey there! This document provides an overview of the compilation pipeline for 
the Ray programming language. It outlines all the AST/IR passes that are performed
during the compilation process. Hopefully, this will help you understand what
to look for when adding new features to the compiler, and where to look when 
debugging issues.

These are the following passes that are performed during compilation in the 
order they are executed:

- **Syntax Tree** - Parses the source code and generates a syntax tree. 
  `rayc_parser` crate contains the parser logic and combinator definitions. The
  syntax tree is defined in `rayc_syntax` crate.
- **Typed AST** - Tree-shaped representation of the function body where each
  expression node is annotated with its type and effect. This is where most of
  the type checking, semantic analysis, and name resolution is performed. 
  The data model for the typed AST is defined in `rayc_typed_ast` crate. The 
  logic that lowers the syntax tree to the typed AST is defined in 
  `rayc_tast_builder` crate.
- **(Mid-Level) IR** - The control-flow graph representation of thefunction body.
  The control-flow of the function body is made explicit in this representation.
  This is where dataflow analysis are performed such as borrow checking. The 
  data model for the IR is defined in `rayc_ir` crate. The logic that lowers the 
  typed AST to the IR is defined in `rayc_ir_builder` crate.
- **Mono IR** - The monomorphized version of the IR. This is where polymorphic
  type variables are instantiated with concrete types and instance dictionaries
  are dispatched. Much of the abstraction is removed (e.g. effects are lowered,
  and closure environments are replaced with concrete type) and the IR closely
  resembles the C representation of the program. The data model for the mono IR 
  is defined in `rayc_mono_ir` crate. The logic that lowers the IR to the mono 
  IR is defined in `rayc_mono_ir_builder` crate.
- **Codegen** - Currently, Mono IR is translated into C code and then compiled
  with **cc** command. C is a pragmatic choice for initial rapid prototyping of 
  the compiler. In the future, we may consider using LLVM or Cranelift for 
  codegen.