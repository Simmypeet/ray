//! Lifetime erasure.
//!
//! Lifetimes only matter up to borrow checking: nothing that consumes the IR
//! afterwards reads them. The last step of building the IR therefore replaces
//! every type of kind lifetime with [`Lifetime::Erased`]: `'static`, lifetime
//! parameters, and the regions the borrow checker's renumbering leaves
//! behind alike.
//!
//! [`Lifetime::Erased`]: rayc_type::ty::lifetime::Lifetime::Erased

use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_function::IRFunctionMap,
    visit::{TypeSite, TypeVisitorMutAsync},
};
use rayc_qbice::TrackedEngine;
use rayc_type::ty::Ty;

/// Replaces every lifetime in `ir`, in place, with an erased lifetime.
pub async fn erase_lifetimes(ir: &mut IRFunctionMap, engine: &TrackedEngine) {
    ir.visit_types_mut_async(&mut LifetimeEraser { engine }).await;
}

/// The [`TypeVisitorMutAsync`] behind [`erase_lifetimes`].
struct LifetimeEraser<'e> {
    engine: &'e TrackedEngine,
}

impl TypeVisitorMutAsync for LifetimeEraser<'_> {
    async fn visit_type_mut_async(&mut self, ty: &mut Interned<Ty>, _: TypeSite) {
        *ty = Ty::erase_lifetimes(ty, self.engine).await;
    }
}
