//! The liveness of the regions of an IR function. Renumbering gives each region
//! of the body one owner, a variable, an expression or an instruction, and the
//! region is live wherever its owner is used later. Where its owner is only
//! dropped later, the region is live only if the drop may use it; see
//! [`DropRegions`].

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local,
    cfg::{Instruction, Point, Terminator},
    ir_expr::IRExprID,
    ir_function::{FunctionID, IRFunction, IRFunctionMap},
    liveness::{LiveMode, LiveRanges, expr::ExprLiveness, local::LocalLiveness},
    visit::{TypeSite, VisitType},
};
use rayc_qbice::TrackedEngine;
use rayc_type::ty::{Ty, lifetime::RegionID};

use crate::drop_check::DropRegions;

/// What keeps a region of a function body live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionOwner {
    /// A local whose type mentions the region.
    Local(Local),

    /// An expression whose value type mentions the region.
    Expression(IRExprID),

    /// A phi, evaluated at this point, whose value type mentions the region.
    /// Its regions are live from the entry of its block.
    Phi(IRExprID, Point),

    /// The instruction or terminator at this point, whose operation uses a
    /// type that mentions the region.
    Instruction(Point),
}

/// Where each region of an IR function is live.
#[derive(Debug, Clone)]
pub struct RegionLiveness {
    /// The owner of each region of the function body. Regions of blocks that
    /// are unreachable have no owner.
    owners: FxHashMap<RegionID, RegionOwner>,

    /// The regions that some drop of their owner may use. An owner that is
    /// only dropped later keeps just these live.
    drop_regions: DropRegions,

    /// Where each local of the function is live.
    locals: LiveRanges<Local>,

    /// Where each expression value of the function is live.
    expressions: LiveRanges<IRExprID>,
}

impl RegionLiveness {
    /// Computes where each region of the function `function_id` is live.
    pub async fn compute(
        ir: &IRFunctionMap,
        function_id: FunctionID,
        engine: &TrackedEngine,
    ) -> Self {
        let function = ir.get_function(function_id);
        let owners = collect_owners(function, function_id);
        let drop_regions = DropRegions::collect(engine, function).await;
        let locals = LocalLiveness::compute(function).await.live_ranges(function);
        let expressions = ExprLiveness::compute(function).await.live_ranges(function);

        Self { owners, drop_regions, locals, expressions }
    }

    /// Returns whether `region` is live at `point`. A universal region is live
    /// everywhere, and a region without an owner nowhere.
    #[must_use]
    pub fn is_live(&self, region: &Interned<Ty>, point: Point) -> bool {
        if region.is_universal_region() {
            return true;
        }

        let Some(region) = region.as_region() else {
            return false;
        };

        self.owners
            .get(&region)
            .is_some_and(|owner| self.keeps_live(region, self.owner_live_mode(*owner, point)))
    }

    /// Returns whether the instruction at `point` uses the value that owns
    /// `region`, or drops it in a way that may use `region`.
    pub(crate) fn is_used_at(
        &self,
        function: &IRFunction,
        region: &Interned<Ty>,
        point: Point,
    ) -> bool {
        let Some(region) = region.as_region() else {
            return false;
        };
        let Some(owner) = self.owners.get(&region) else {
            return false;
        };

        let mode = match *owner {
            RegionOwner::Local(local) => LocalLiveness::use_at(function, point, local),
            RegionOwner::Expression(expression_id) | RegionOwner::Phi(expression_id, _) => {
                ExprLiveness::use_at(function, point, expression_id)
            }
            RegionOwner::Instruction(instruction_point) => {
                (instruction_point == point).then_some(LiveMode::Use)
            }
        };

        self.keeps_live(region, mode)
    }

    /// Returns how `owner` is live at `point`, if it is. An instruction is
    /// only live at its own point, and uses every region it owns there.
    fn owner_live_mode(&self, owner: RegionOwner, point: Point) -> Option<LiveMode> {
        match owner {
            RegionOwner::Local(local) => self.locals.live_mode(local, point),
            RegionOwner::Expression(expression_id) => {
                self.expressions.live_mode(expression_id, point)
            }

            // The incoming values are held from the terminators of the
            // predecessors until the phi is evaluated.
            RegionOwner::Phi(expression_id, phi_point) => {
                let is_pending = point.block_id() == phi_point.block_id()
                    && point.instruction_idx() <= phi_point.instruction_idx();

                if is_pending {
                    Some(LiveMode::Use)
                } else {
                    self.expressions.live_mode(expression_id, point)
                }
            }
            RegionOwner::Instruction(instruction_point) => {
                (instruction_point == point).then_some(LiveMode::Use)
            }
        }
    }

    /// Returns whether an owner live in `mode` keeps `region` live: a later
    /// use keeps every region of the owner live, and a later drop only those
    /// it may use.
    fn keeps_live(&self, region: RegionID, mode: Option<LiveMode>) -> bool {
        match mode {
            Some(LiveMode::Use) => true,
            Some(LiveMode::Drop) => self.drop_regions.contains(region),
            None => false,
        }
    }
}

/// Returns the owner of each region of the body of `function`.
fn collect_owners(
    function: &IRFunction,
    function_id: FunctionID,
) -> FxHashMap<RegionID, RegionOwner> {
    let mut collector =
        OwnerCollector { site: TypeSite::Body(function_id), owners: FxHashMap::default() };

    for (variable_id, variable) in function.variables() {
        collector.record(variable.ty(), RegionOwner::Local(Local::Variable(variable_id)));
    }

    let reachables = function.reachables();
    for block_id in reachables.blocks() {
        for (point, instruction) in function.block_instructions_with_points(block_id) {
            collector.record_instruction(function, point, instruction);
        }

        // A suspension point evaluates its expression at its terminator,
        // and a resumed coroutine defines what it gives its successors
        // there.
        let point = function.terminator_point(block_id);
        let evaluated = function
            .block_terminator(block_id)
            .into_iter()
            .flat_map(Terminator::evaluated_expressions);
        for expression_id in evaluated {
            collector.record_expression(function, point, expression_id);
        }
    }

    collector.owners
}

/// Records the owner of each region of a function body for
/// [`collect_owners`].
struct OwnerCollector {
    /// The site of the function body's types.
    site: TypeSite,

    owners: FxHashMap<RegionID, RegionOwner>,
}

impl OwnerCollector {
    /// Records the owners of the regions that the instruction at `point`
    /// introduces.
    fn record_instruction(
        &mut self,
        function: &IRFunction,
        point: Point,
        instruction: &Instruction,
    ) {
        match instruction {
            Instruction::Expression(expression_id) => {
                self.record_expression(function, point, *expression_id);
            }

            Instruction::ExprDiscard(discard) => {
                self.record(discard.drop_instance(), RegionOwner::Instruction(point));
            }
            Instruction::AddressDrop(drop) => {
                self.record(drop.drop_instance(), RegionOwner::Instruction(point));
            }

            // Neither introduces a type.
            Instruction::Store(_) | Instruction::ScopePush(_) | Instruction::ScopePop(_) => {}
        }
    }

    /// Records the owners of the regions that evaluating `expression_id` at
    /// `point` introduces.
    ///
    /// The type of the value belongs to the expression, and every other type
    /// its operation uses to the instruction or terminator at `point`.
    fn record_expression(&mut self, function: &IRFunction, point: Point, expression_id: IRExprID) {
        let expression = function.get_expression(expression_id);
        let owner = if expression.kind().as_phi().is_some() {
            RegionOwner::Phi(expression_id, point)
        } else {
            RegionOwner::Expression(expression_id)
        };
        self.record(expression.ty(), owner);
        expression.kind().visit_types(self.site, &mut |ty: &Interned<Ty>, _| {
            self.record(ty, RegionOwner::Instruction(point));
        });
    }

    /// Records `owner` as the owner of every region in `ty`.
    fn record(&mut self, ty: &Ty, owner: RegionOwner) {
        for region in ty.recursive_iter().filter_map(Ty::as_region) {
            // Renumbering gives each lifetime occurrence its own region, so
            // no region can have two owners.
            let previous = self.owners.insert(region, owner);
            debug_assert!(
                previous.is_none_or(|previous| previous == owner),
                "region {region:?} has two owners: {previous:?} and {owner:?}"
            );
        }
    }
}
