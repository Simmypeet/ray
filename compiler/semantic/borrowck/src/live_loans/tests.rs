use crate::test_util::{FunctionBuilder, running_example};

// input: liveness of `L1` at `y = ..` in `bb4`
// premise: the running example: `L1` flows from `q` into `p` in `bb2`, and
//          `p` is used after `y = ..` in `bb4`
// output: `L1` is live
#[tokio::test]
async fn loan_is_live_where_it_flows_into_a_region_used_later() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l1 = analysis.loan_at(example.borrow_y);

    assert!(analysis.is_live(l1, example.y_before_use));
}

// input: liveness of `L1` at `y = ..` in `bb3`
// premise: the running example: `q` is dead in `bb3`, and `p` there only
//          holds `L0`
// output: `L1` is not live, although `bb3` reaches the use of `p` in `bb4`
#[tokio::test]
async fn loan_is_not_live_on_a_path_where_no_live_region_holds_it() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l1 = analysis.loan_at(example.borrow_y);

    assert!(!analysis.is_live(l1, example.y_on_else_path));
}

// input: liveness of `L0` at `x = ..` in `bb2`
// premise: the running example: `p = q` overwrites `p`, the only holder of
//          `L0`, at the start of `bb2`
// output: `L0` is not live
#[tokio::test]
async fn loan_is_not_live_after_its_holder_is_overwritten() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l0 = analysis.loan_at(example.borrow_x);

    assert!(!analysis.is_live(l0, example.x_after_reassign));
}

// input: liveness of `L0` at `y = ..` in `bb3`
// premise: the running example: `p` still holds `L0` in `bb3`, and is used
//          in `bb4`
// output: `L0` is live
#[tokio::test]
async fn loan_stays_live_across_blocks_while_its_holder_is_live() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let analysis = builder.analyze().await;
    let l0 = analysis.loan_at(example.borrow_x);

    assert!(analysis.is_live(l0, example.y_on_else_path));
}
