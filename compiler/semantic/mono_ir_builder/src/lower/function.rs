use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::cfg::{Instruction as IRInstruction, Store};
use rayc_mono_ir::operand::Operand;

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers the function in callback style.
    pub(crate) async fn lower_function(mut self, ctx: &Context<'_>) {
        self.lower_blocks(ctx).await;
    }

    /// Lowers every block that direct control flow reaches.
    pub(super) async fn lower_blocks(&mut self, ctx: &Context<'_>) {
        for source_block in ctx.source_direct_reachables().blocks() {
            self.select_block(ctx.block(source_block));
            for instruction in ctx.source_block_instructions(source_block) {
                self.lower_instruction(instruction, ctx).await;
            }

            let terminator = ctx
                .source_block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            self.lower_terminator(source_block, terminator, ctx).await;
        }
    }

    pub(super) async fn lower_instruction(
        &mut self,
        instruction: &IRInstruction,
        ctx: &Context<'_>,
    ) {
        match instruction {
            IRInstruction::ScopePush(_) | IRInstruction::ScopePop(_) => {}
            IRInstruction::ExprDiscard(discard) => self.lower_expr_discard(discard, ctx).await,
            IRInstruction::AddressDrop(drop) => self.lower_address_drop(drop, ctx).await,
            IRInstruction::Expression(expression_id) => {
                self.lower_expression(*expression_id, ctx).await;
            }
            IRInstruction::Store(store) => self.lower_store(store, ctx),
        }
    }

    /// Lowers `store`, which moves the value of an expression into a place.
    ///
    /// A boxed `coro` storage is first written here, so its box is allocated
    /// just before. A boxed `Coroutine` expression is moved out of its box,
    /// which is freed just after.
    ///
    /// In theory, the expression is never boxed. `coro` stores it right after
    /// it is evaluated, in the same block, so it never lives across a
    /// suspension point and the frame never holds it. Its box is handled
    /// anyway, so that nothing depends on how `coro` is lowered.
    ///
    /// ```c
    ///     e_box = frame->e;                                // a boxed expression only
    ///     frame->storage = ray_alloc(sizeof(c_frame_t));   // a boxed storage only
    ///     *frame->storage = *e_box;                        // or `e`, unboxed
    ///     ray_free(e_box);                                 // a boxed expression only
    /// ```
    fn lower_store(&mut self, store: &Store, ctx: &Context<'_>) {
        let expression = store.expression();

        // The member of the expression may share memory with the storage,
        // which the store is the first to write. So the pointer to a boxed
        // expression is read before anything is written.
        let expression_box = self.take_expression_box(expression, ctx);
        let value = expression_box.as_ref().map_or_else(
            || ctx.expression_operand(expression),
            |pointer| Operand::Copy(self.project(pointer).dereference()),
        );

        // Allocating writes the member of the storage, so it must not share
        // memory with a value that the frame holds by value. Only the
        // `Coroutine` expression of a `coro` is stored there, and the frame
        // never holds it by value.
        if let Some(storage) = store.address().whole_local()
            && self.allocate_frame_box(Savable::Local(storage), ctx).is_some()
        {
            assert!(
                expression_box.is_some() || !ctx.frame_holds(Savable::Expression(expression)),
                "a value stored into a boxed local should not be held in the frame by value"
            );
        }

        let destination = self.lower_address(store.address(), ctx);
        self.store(destination, value);

        // The value has moved out of the box.
        if let Some(pointer) = expression_box {
            self.heap_free(pointer);
        }
    }
}
