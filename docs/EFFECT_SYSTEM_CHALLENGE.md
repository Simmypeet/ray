# The Current State of Effect System 

Currently, the effect system is implemented following the design of Koka's 
effect rows. This gives us:

- A very straightforward extension to the type system, the effect system is just 
  an another row-kind added, and the inference system is just a simple 
  well-studied row unification algorithm.
- It surprisingly handles **lifetime varaince** and **subtyping** very well, 
  which is an another property required in a language with borrow-checker
  and lifetime tracking. This is due to its simple yet effective row unification
  algorithm.

> Lifetime variance and subtyping is best illustrated as "`Vector[&'static T]` 
  can be used (supertype) in place of `Vector[&'a T]` (subtype) for any lifetime 
  `'a`". In the lens of effect system, this is equivalent to "`{Reader[&'a T]}`
  can be performed in a function that allows `{Reader[&'b T]}` as long as 
  `'a: 'b` (assuming `Reader` is a covariant effect). 


That's said, it exhibits two unwanted properties that we would like to address, 
we'll introduce to them below and discuss them in detail in the next section.

- Composibility: The current effect syntax cannot compose two different 
opaque effect rows. This means there's no trivial way to represent "union" of
two different effect rows. Often, the workaround is to "widen" those two effect
rows to a common "widened" effect row, which leads to the next problem.
- Widening: The current effect system is not very precise. For instance, if we
define a closure that in-fact only performs a single effect, but due to the
earlier mentioned composibility problem, we have to widen the effect row to a 
more general one. This is often not an issue, but imprecision in effect typing
can lead to subpar code generation performance.

# Composibility 

To understand the composibility problem, we must understand how two effect
rows are represented in the current effect system. Using Rust syntax, this
is currently represented as:

```rust
/// Represents the type of kind `Effect` 
pub enum EffectRow {
    Empty,
    InferenceVar(InferenceVar),
    AssociatedEff(AssociatedEff),
    EffParam(EffParam),
    Row(Row)
}

/// Think of a single effect instance like `State[int32]` or `Yield[float]` as 
/// a label in the effect row.
pub struct EffectLabel {
    pub symbol: GlobalSymbolID,
    pub args: Args,
}

/// A collection of effect labels, with a **single** effect row tail
pub struct Row {
    pub labels: Vec<EffectLabel>,
    pub tail: Box<EffectRow>,
}
```

Notice, that the `Row` struct has only a single `tail` field, which makes it
impossible to compose two or more different *opaque effect rows*. Opaque effect
rows in this case are represented by the `InferenceVar`, `AssociatedEff`, 
and `EffParam`.

This is a motivating example of the composibility problem. Consider the 
following code written in Ray source code:

```ray
pub eff Reader[a]:
    type Eff: Effect

    fn read(self: a) -> int32 \ this.Eff

pub inst FileReader for Reader[File]:
    type Eff = {FileSystem}

    fn read(self: File) -> int32 \ {FileSystem}


pub inst NetworkReader for Reader[Network]:
    type Eff = {Network}

    fn read(self: Network) -> int32 \ {Network}


pub inst CombineReaders for Reader[(a, b)] given (rA: Reader[a], rB: Reader[b]):
    type Eff = rA.Eff + rB.Eff # this is currently not possible

    fn read(self: (a, b)) -> int32 \ this.Eff:
        let (readerA, readerB) = self
        let resultA = rA.read(readerA)
        let resultB = rB.read(readerB)
        return resultA + resultB


pub def readFromReaders(
    readerA: a, readerB: b
) given (rA: Reader[a], rB: Reader[b]) -> int32 \ rA.Eff + rB.Eff: # not possible
    rA.read(readerA)
    rB.read(readerB)
```

The function `readFromReaders` attempts to compose two different opaque 
effect rows, `rA.Eff` and `rB.Eff` through the hypothetical `+` operator.
Obviously, this is not possible in the current effect syntax as described above,
this is simply what the composibility problem is.

## Current Workarounds

Again, the common workaround is to "widen" the effect rows to a common effect
row, which is often a more general effect row. Here's how we can rewrite the 
above example to use a common effect row:

```ray
pub eff Reader[a, e: Effect]:
    fn read(self: a) -> int32 \ e


pub inst FileReader[e] for Reader[File, {FileSystem | e}]:
    fn read(self: File) -> int32 \ {FileSystem | e}


pub inst NetworkReader[e] for Reader[Network, {Network | e}]:
    fn read(self: Network) -> int32 \ {Network | e}


pub inst CombineReaders[e] for Reader[(a, b), e] given (rA: Reader[a, e], rB: Reader[b, e]):
    fn read(self: (a, b)) -> int32 \ e:
        let (readerA, readerB) = self
        let resultA = rA.read(readerA)
        let resultB = rB.read(readerB)
        return resultA + resultB


pub def readFromReaders[e](
    readerA: a, readerB: b
) given (rA: Reader[a, e], rB: Reader[b, e]) -> int32 \ e:
    rA.read(readerA)
    rB.read(readerB)
```

In this example, we have added a new effect parameter `e` to the `Reader` 
representing the effect row that the `Reader.read` function will perform. 
The `FileReader` and `NetworkReader` instances now accept an additional effect
parameter `e`, which is used to widen the effect row, and implements the 
`Reader` trait with the widened effect row `{MyEffect | tail}`.

The `readFromReaders` function now only performs a single effect row `\ e`,  
which is the common effect row that both `rA` and `rB` can perform. 

This deals with the composibility problem, however, we introduce:

- Trait associated effect: The `Reader` trait now can't utilize the associated
effect `Reader[a].Eff` like the oringinal example anymore, instead, it has to 
declare an additional effect parameter `e` to represent the effect row. This
is generally not a problem in this case, but imaine if we have a trait with 
many functions, and each function has a different effect row, then the signature
of the trait will become very cluttered with effect parameters (for instance, 
`Reader[a, eForRead, eForWrite, eForClose, ...]`).
- Widening: In this particular example, if we want to use both `FileReader` and
`NetworkReader` in the same function, we have to widen the effect row of 
`FileReader` by instantiating it with `FileReader[{Network}]` so that the 
it implements `Reader[File, {FileSystem | Network}]` and thus can be used
together with `NetworkReader` in the same function. This means that, the
`FileReader.read` function will perform both `FileSystem` and `Network` effects, 
even though it only performs `FileSystem` effect. This is the imprecision 
problem we mentioned earlier.

## Alternative System Considered: Flix's Set Formula

Flix utilizes a set formula to represent effect rows, which has a strong 
composibility property. Its effect syntax even allows composing multiple effect
**formulas** together with multiple operators, such as union, intersection, 
and difference. It allows for a very precise effect typing and can compose
multiple effect formulas together effortlessly. However, it lacks (or at least,
I haven't studied it enough to find out) a well-defined subtyping and variance 
property.

## Complete new System?

