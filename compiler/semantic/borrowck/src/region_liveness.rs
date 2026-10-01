//! The liveness of the regions of an IR function.
//!
//! A region is live at a point when a value whose type mentions it may still
//! be used or dropped there. The traversal of the localized constraint graph
//! asks this once for each step it takes, so it is answered from the owner of
//! the region rather than from a table of every region at every point.
//!
//! [Renumbering](crate::renumber) gives each lifetime occurrence in a
//! function body its own region, so each region has exactly one owner:
//!
//! - a variable, whose type mentions it: the region is live wherever the
//!   variable is;
//! - an expression, whose value type mentions it: the region is live wherever
//!   the value is;
//! - an instruction, whose operation uses a type that no value holds, such as
//!   the substitution of a call or a `Drop` dictionary: the region is live only
//!   at that instruction.
//!
//! Every other lifetime is universal: `'static`, a lifetime parameter, or an
//! external lifetime, which covers the types of parameters and captures. A
//! universal region outlives the function body, so it is live everywhere.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local,
    cfg::{Instruction, Point},
    ir_expr::IRExprID,
    ir_function::{FunctionID, IRFunction, IRFunctionMap},
    liveness::{LiveRanges, expr::ExprLiveness, local::LocalLiveness},
    visit::{TypeSite, VisitType},
};
use rayc_type::ty::{Ty, lifetime::RegionID};

/// What keeps a region of a function body live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionOwner {
    /// A local whose type mentions the region.
    Local(Local),

    /// An expression whose value type mentions the region.
    Expression(IRExprID),

    /// The instruction at this point, whose operation uses a type that
    /// mentions the region.
    Instruction(Point),
}

/// Where each region of an IR function is live.
#[derive(Debug, Clone)]
pub struct RegionLiveness {
    /// The owner of each region of the function body. Regions of blocks that
    /// are unreachable have no owner.
    owners: FxHashMap<RegionID, RegionOwner>,

    /// Where each local of the function is live.
    locals: LiveRanges<Local>,

    /// Where each expression value of the function is live.
    expressions: LiveRanges<IRExprID>,
}

impl RegionLiveness {
    /// Computes where each region of the function `function_id` is live.
    ///
    /// This is meant to run after [renumbering](crate::renumber).
    pub async fn compute(ir: &IRFunctionMap, function_id: FunctionID) -> Self {
        let function = ir.get_function(function_id);
        let owners = collect_owners(function, function_id);
        let locals = LocalLiveness::compute(function).await.live_ranges(function);
        let expressions = ExprLiveness::compute(function).await.live_ranges(function);

        Self { owners, locals, expressions }
    }

    /// Returns whether `region` is live at `point`.
    ///
    /// Any lifetime other than a region variable is universal to the
    /// function, and is live everywhere. A region variable is live wherever
    /// its owner is, and never when it has none, since its code is
    /// unreachable.
    #[must_use]
    pub fn is_live(&self, region: &Interned<Ty>, point: Point) -> bool {
        let Some(region) = region.as_region() else {
            // if reaches `else` here, it means the region is universal, so it is live
            // everywhere
            return true;
        };

        self.owners.get(&region).is_some_and(|owner| self.is_owner_live(*owner, point))
    }

    /// Returns whether the instruction at `point` uses or drops the value
    /// that owns `region`, which may still hold the loans in the region.
    ///
    /// A universal region, or one without an owner, belongs to no value of
    /// the function, so no instruction uses it.
    pub(crate) fn is_used_at(
        &self,
        function: &IRFunction,
        region: &Interned<Ty>,
        point: Point,
    ) -> bool {
        let Some(owner) = region.as_region().and_then(|region| self.owners.get(&region)) else {
            return false;
        };

        match *owner {
            RegionOwner::Local(local) => LocalLiveness::use_at(function, point, local).is_some(),
            RegionOwner::Expression(expression_id) => {
                ExprLiveness::use_at(function, point, expression_id).is_some()
            }
            RegionOwner::Instruction(instruction_point) => instruction_point == point,
        }
    }

    /// Returns whether `owner` keeps its regions live at `point`.
    fn is_owner_live(&self, owner: RegionOwner, point: Point) -> bool {
        // TODO: a value that is only dropped keeps every region of its type
        // live, as if it were used. Drop check would keep only the regions
        // its `Drop` implementation may use.
        match owner {
            RegionOwner::Local(local) => self.locals.live_mode(local, point).is_some(),
            RegionOwner::Expression(expression_id) => {
                self.expressions.live_mode(expression_id, point).is_some()
            }
            RegionOwner::Instruction(instruction_point) => instruction_point == point,
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
            // The value's type belongs to the expression, and every other
            // type its operation uses belongs to the instruction evaluating
            // it.
            Instruction::Expression(expression_id) => {
                let expression = function.get_expression(*expression_id);
                self.record(expression.ty(), RegionOwner::Expression(*expression_id));
                expression.kind().visit_types(self.site, &mut |ty: &Interned<Ty>, _| {
                    self.record(ty, RegionOwner::Instruction(point));
                });
            }

            Instruction::ExprDiscard(discard) => {
                self.record(discard.drop_instance(), RegionOwner::Instruction(point));
            }
            Instruction::AddressDrop(drop) => {
                self.record(drop.drop_instance(), RegionOwner::Instruction(point));
            }

            // Neither introduces a type: a store writes a value that has one
            // already.
            Instruction::Store(_) | Instruction::ScopePush(_) | Instruction::ScopePop(_) => {}
        }
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
