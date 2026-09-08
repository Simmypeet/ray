# Ray 

This is the "rewrite" of an existing language [Pernix](https://github.com/Simmypeet/pernix-lang) into a new language called Ray as a fresh start.

The goal is to see whether we can have a language that is:

- System-level programming language, same performance as C/C++/Rust
- Has a memory-safe type system, like borrow checker in Rust
- Has an effect system that tracks side effects, like in Koka or Flix
- Has an effect handler with predictable performance and efficient implementation
- Has a trait/typeclass system but in an explicit dictionary-passing style like OCaml modules and Scala implicits
- Explores the new rules in borrow checking that enables more flexible ownership and borrowing while still being memory-safe. (Possibly comes with more complexity in the type system, but we can see whether we can make it's ergonomic enough to be usable)

:P