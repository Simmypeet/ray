use tokio::sync::oneshot;

use super::JoinList;

#[tokio::test]
async fn next_follows_spawn_order_rather_than_completion_order() {
    let (second_finished, wait_for_second) = oneshot::channel();

    // the first task can finish only after the second one has
    let mut list = JoinList::new();
    list.spawn(async move {
        wait_for_second.await.unwrap();
        "first"
    });
    list.spawn(async move {
        second_finished.send(()).unwrap();
        "second"
    });

    assert_eq!(list.next().await, Some("first"));
    assert_eq!(list.next().await, Some("second"));
    assert_eq!(list.next().await, None);
}

#[tokio::test]
async fn dropping_aborts_unjoined_tasks() {
    let (held_by_task, task_dropped) = oneshot::channel::<()>();

    // the task never finishes on its own, keeping the sender alive
    let mut list = JoinList::new();
    list.spawn(async move {
        let _held_by_task = held_by_task;
        std::future::pending::<()>().await;
    });

    drop(list);

    // the receiver is notified only once the aborted task drops the sender
    assert!(task_dropped.await.is_err());
}
