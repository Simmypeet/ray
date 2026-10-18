//! Drop check: which regions of a value its drop may use.
//!
//! A value that is only dropped later keeps alive just the regions that its
//! `Drop` dictionary can reach. Dropping a `Wrapper[&'r int32]` with a
//! generated dictionary only drops the reference, which does nothing, so `'r`
//! need not stay live until the drop. Dropping a struct with a declared
//! `Drop` instance may use anything the value reaches, so every region of it
//! does.
//!
//! The walk follows the dropped type together with its dictionary. Only the
//! regions of the dropped type are collected: the dictionary is renumbered
//! separately, so only its shape is read.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_ir::{
    address::{Local, Projection},
    cfg::{AddressDrop, Instruction},
    ir_function::IRFunction,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    drop_plan::{DropPlan, get_drop_plan},
    struct_body::get_struct_body,
};
use rayc_type::{
    subst::Substitutable,
    ty::{Ty, application::View as ApplicationView, lifetime::RegionID},
};

/// The regions of a function body that some drop of their owner may use.
#[derive(Debug, Clone, Default)]
pub struct DropRegions {
    regions: FxHashSet<RegionID>,
}

impl DropRegions {
    /// Collects the regions that the drops in the reachable blocks of
    /// `function` may use.
    pub async fn collect(engine: &TrackedEngine, function: &IRFunction) -> Self {
        let mut collector = DropRegionCollector {
            engine,
            visited: FxHashSet::default(),
            regions: FxHashSet::default(),
        };

        let reachables = function.reachables();
        for block_id in reachables.blocks() {
            for instruction in function.block_instructions(block_id) {
                collector.record_instruction(function, instruction).await;
            }
        }

        Self { regions: collector.regions }
    }

    /// Returns whether some drop of the owner of `region` may use it.
    #[must_use]
    pub fn contains(&self, region: RegionID) -> bool { self.regions.contains(&region) }
}

/// Walks the drops of a function body for [`DropRegions::collect`].
struct DropRegionCollector<'e> {
    engine: &'e TrackedEngine,

    /// The pairs of a dropped type and its dictionary walked already. Walking
    /// one again would find the same regions.
    visited: FxHashSet<(Interned<Ty>, Interned<Ty>)>,

    regions: FxHashSet<RegionID>,
}

impl DropRegionCollector<'_> {
    /// Records the regions that the drop performed by `instruction`, if any,
    /// may use.
    async fn record_instruction(&mut self, function: &IRFunction, instruction: &Instruction) {
        match instruction {
            Instruction::AddressDrop(drop) => self.record_place_drop(function, drop).await,
            Instruction::ExprDiscard(discard) => {
                let ty = function.get_expression(discard.expression()).ty().clone();
                self.record_drop(&ty, discard.drop_instance()).await;
            }

            // None of them drops a value.
            Instruction::Expression(_)
            | Instruction::Store(_)
            | Instruction::ScopePush(_)
            | Instruction::ScopePop(_) => {}
        }
    }

    /// Records the regions that dropping the place of `drop` may use.
    ///
    /// Only a variable owns the regions of its type: those of parameters and
    /// captures are universal or external, and live everywhere anyway.
    async fn record_place_drop(&mut self, function: &IRFunction, drop: &AddressDrop) {
        let Some(Local::Variable(variable_id)) = drop.address().local() else {
            return;
        };

        let variable_ty = function.get_variable(variable_id).ty().clone();
        match self.dropped_place_type(&variable_ty, drop.address().projections()).await {
            Some(place_ty) => self.record_drop(&place_ty, drop.drop_instance()).await,

            // A place behind a pointer is not owned by the variable; whatever
            // the drop does, it goes through the pointer.
            None => self.record_all(&variable_ty),
        }
    }

    /// Returns the type of the component of `ty` that `projections` select,
    /// or `None` when they dereference a pointer.
    ///
    /// The type is not normalized: a component whose shape does not match
    /// its dictionary is treated conservatively by [`Self::record_drop`].
    async fn dropped_place_type(
        &self,
        ty: &Interned<Ty>,
        projections: &[Projection],
    ) -> Option<Interned<Ty>> {
        let mut ty = ty.clone();

        for projection in projections {
            ty = match projection {
                Projection::Tuple(index) => ty.as_tuple_view()?.args().get(*index)?.clone(),

                // Substituting the variable's own type arguments keeps the
                // regions of its type.
                Projection::Field(field_id) => {
                    let struct_ = ty.as_struct_view()?;
                    let substitution = struct_.create_subst(self.engine).await;
                    let body = self.engine.get_struct_body(struct_.symbol_id()).await;
                    body[*field_id].ty().apply_subst_or_clone(&substitution, self.engine)
                }

                Projection::Deref | Projection::RawDeref => return None,
            };
        }

        Some(ty)
    }

    /// Records the regions of `ty` that dropping a value of `ty` with
    /// `dictionary` may use.
    #[allow(clippy::match_same_arms)]
    async fn record_drop(&mut self, ty: &Interned<Ty>, dictionary: &Interned<Ty>) {
        if !self.visited.insert((ty.clone(), dictionary.clone())) {
            return;
        }

        Box::pin(async move {
            let Ty::Application(application) = &**dictionary else {
                // A dictionary given to the current function may do anything
                // with the value.
                self.record_all(ty);
                return;
            };

            match application.view() {
                // Nothing runs.
                ApplicationView::NoOpDropInstance(_) => {}

                // Each element is dropped with its own dictionary.
                ApplicationView::TupleDropInstance(view) => {
                    let Some(tuple) = ty.as_tuple_view() else {
                        self.record_all(ty);
                        return;
                    };

                    for (element, dictionary) in tuple.args().iter().zip(view.element_instances()) {
                        self.record_drop(element, dictionary).await;
                    }
                }

                // Only the captures are dropped, each with its own dictionary;
                // the rest of the closure type is never touched.
                ApplicationView::ClosureDropInstance(view) => {
                    let Some(captured) = ty
                        .as_closure_view()
                        .and_then(|closure| closure.captured_tuple().as_tuple_view())
                    else {
                        self.record_all(ty);
                        return;
                    };

                    for (capture, dictionary) in
                        captured.args().iter().zip(view.capture_instances())
                    {
                        self.record_drop(capture, dictionary).await;
                    }
                }

                // A generated `Drop` drops the fields as its plan says, which
                // is exactly what the generated body does.
                ApplicationView::NominalDropInstance(view) => {
                    self.record_generated_drop(ty, view.external_instances()).await;
                }

                // A declared instance may use anything the value reaches, and
                // a suspended coroutine holds locals its type does not show.
                ApplicationView::Instance(_) | ApplicationView::CoroutineDropInstance(_) => {
                    self.record_all(ty);
                }

                // Not `Drop` dictionaries; drop elaboration never picks them,
                // but treating them conservatively is always sound.
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Coroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::Error => self.record_all(ty),
            }
        })
        .await;
    }

    /// Records the regions of the struct `ty` that its generated `Drop`, with
    /// `external_instances` for the plan's requirements, may use.
    async fn record_generated_drop(
        &mut self,
        ty: &Interned<Ty>,
        external_instances: &[Interned<Ty>],
    ) {
        let Some(struct_) = ty.as_struct_view() else {
            self.record_all(ty);
            return;
        };
        let plan = self.engine.get_drop_plan(struct_.symbol_id()).await;
        let DropPlan::Generated(plan) = &*plan else {
            self.record_all(ty);
            return;
        };

        // Substituting the dropped type's own arguments into the field types
        // keeps its regions, so the fields' regions are the ones collected.
        let substitution = struct_.create_subst(self.engine).await;
        let body = self.engine.get_struct_body(struct_.symbol_id()).await;

        for field in plan.fields() {
            let field_ty =
                body[field.field_id()].ty().apply_subst_or_clone(&substitution, self.engine);
            let dictionary =
                field.dictionary().instantiate(&substitution, external_instances, self.engine);

            self.record_drop(&field_ty, &dictionary).await;
        }
    }

    /// Records every region of `ty`.
    fn record_all(&mut self, ty: &Ty) {
        self.regions.extend(ty.recursive_iter().filter_map(Ty::as_region));
    }
}
