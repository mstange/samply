use samply_markers::prelude::*;
use tokio::time::{Duration, sleep};

async fn migrating_task(task_id: usize, iterations: usize) {
    for i in 0..iterations {
        samply_measure!(async {
            samply_marker!({ name: format!("task({task_id}): iteration {i} start") });

            sleep(Duration::from_micros(100)).await;

            let mut sum = 0u64;
            for j in 0..10_000 {
                sum = sum.wrapping_add(j);
            }

            sleep(Duration::from_micros(100)).await;
        }, marker: {
            name: format!("task {task_id} iter {i}"),
        })
        .await;

        // Yield to encourage task migration.
        tokio::task::yield_now().await;
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() {
    let _main_timer = samply_timer!({ name: "main()" });

    println!("\n=== Tokio Task Migration Demo ===\n");
    println!("This example spawns async tasks on a multi-threaded runtime.");
    println!("Tasks will migrate between worker threads during execution.");

    let num_tasks = 8;
    let iterations_per_task = 20;

    samply_marker!({ name: "spawning tasks" });

    let handles = (0..num_tasks)
        .map(|task_id| {
            tokio::spawn(async move { migrating_task(task_id, iterations_per_task).await })
        })
        .collect::<Vec<_>>();

    samply_marker!({ name: "all tasks spawned" });

    samply_measure!({
        futures::future::join_all(handles).await
    }, marker: {
        name: "waiting for all tasks",
    });

    println!("\nAll tasks completed!");
}
