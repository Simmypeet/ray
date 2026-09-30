use rayc_ir::cfg::{Point, Terminator};

use super::{ActiveLoans, LoanActivity};
use crate::{
    constraint::LoanID,
    test_util::{Analysis, FunctionBuilder, running_example},
};

/// Returns the loans active just before the instruction at `point`, which
/// its accesses are checked against.
///
/// This replays the block of `point` from its solved entry fact, so it is
/// only meant for checking one point at a time.
fn active_before(activity: &LoanActivity<'_>, point: Point) -> ActiveLoans {
    let problem = &activity.problem;
    let mut state = activity
        .solution
        .block_entry(point.block_id())
        .expect("the point should be reachable")
        .clone();

    for (earlier, instruction) in problem
        .function
        .block_instructions_with_points(point.block_id())
        .take(point.instruction_idx())
    {
        problem.kill_dead_loans(earlier, &mut state);
        problem.apply_instruction(instruction, &mut state);
    }

    problem.kill_dead_loans(point, &mut state);
    state
}

/// Returns whether `loan` is active just before the instruction at `point`.
async fn is_active_before(analysis: &Analysis, loan: LoanID, point: Point) -> bool {
    let activity = analysis.loan_activity().await;
    active_before(&activity, point).contains(loan)
}

// input: whether `L1` is active before `y = ..` in `bb4`
// premise: the running example: `L1` is live there, since it flows into `p`,
//          which is used afterwards
// output: `L1` is active
#[tokio::test]
async fn live_loan_is_active_before_a_later_access() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l1 = analysis.loan_at(example.borrow_y);

    assert!(is_active_before(&analysis, l1, example.y_before_use).await);
}

// input: whether `L1` is active before `y = ..` in `bb3`
// premise: the running example: `L1` is issued on every path into `bb3`, but
//          is not live there
// output: `L1` is not active
#[tokio::test]
async fn loan_is_not_active_where_it_is_not_live() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l1 = analysis.loan_at(example.borrow_y);

    assert!(!is_active_before(&analysis, l1, example.y_on_else_path).await);
}

// input: whether `L` is active after `x = ..`
// premise: p = &x.0 (L); x = ..; use(p); return
// output: `L` is not active, although `p` is used later: overwriting `x`
//         overwrites `x.0`
#[tokio::test]
async fn overwriting_a_place_ends_the_loans_of_places_within_it() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable(builder.pair());
    let p = builder.variable(builder.reference());
    let entry = builder.entry();

    let borrow = builder.borrow_into(entry, p, builder.element(x, 0));
    builder.write(entry, builder.address(x), builder.pair());
    let after_write = builder.next_point(entry);
    builder.read(entry, p);
    builder.terminate(entry, Terminator::Return(None));

    let analysis = builder.analyze().await;
    let loan = analysis.loan_at(borrow);
    assert!(!is_active_before(&analysis, loan, after_write).await);
}

// input: whether `L` is active after `x.1 = ..`
// premise: p = &x.0 (L); x.1 = ..; use(p); return
// output: `L` is active: `x.1` does not contain `x.0`
#[tokio::test]
async fn overwriting_a_sibling_place_keeps_the_loan_active() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable(builder.pair());
    let p = builder.variable(builder.reference());
    let entry = builder.entry();

    let borrow = builder.borrow_into(entry, p, builder.element(x, 0));
    builder.write(entry, builder.element(x, 1), builder.int32());
    let after_write = builder.next_point(entry);
    builder.read(entry, p);
    builder.terminate(entry, Terminator::Return(None));

    let analysis = builder.analyze().await;
    let loan = analysis.loan_at(borrow);
    assert!(is_active_before(&analysis, loan, after_write).await);
}

// input: whether `L` is active after the scope of `x` ends
// premise: { let x; p = &x (L); } use(p); return
// output: `L` is not active, although `p` is used later: the storage of `x`
//         is gone
#[tokio::test]
async fn scope_end_ends_the_loans_of_its_variables() {
    let mut builder = FunctionBuilder::new().await;
    let scope = builder.scope();
    let x = builder.variable_in(scope, builder.int32());
    let p = builder.variable(builder.reference());
    let entry = builder.entry();

    builder.push_scope(entry, scope);
    let borrow = builder.borrow_into(entry, p, builder.address(x));
    builder.pop_scope(entry, scope);
    let after_scope = builder.next_point(entry);
    builder.read(entry, p);
    builder.terminate(entry, Terminator::Return(None));

    let analysis = builder.analyze().await;
    let loan = analysis.loan_at(borrow);
    assert!(!is_active_before(&analysis, loan, after_scope).await);
}
