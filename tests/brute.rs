const NUM_ITERATIONS: usize = 4000;

use std::thread;
use std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
fn main() {

    logwise::warn_sync!("=== submit_to_main_thread Latency Benchmark ===");

    app_window::application::main(|| {
        thread::spawn(|| {
            let t = Task::without_notifications(
                "submit_to_main_thread_benchmark".to_string(),
                Configuration::default(),
                async {
                    run_benchmark().await;
                },
            );
            t.spawn_static_current();
        });
    });
}


#[test_executors::async_test] async fn brute() {
    let mut senders = Vec::new();
    let mut futures = Vec::new();
    for _ in 0..NUM_ITERATIONS {
        let (tx, rx) = r#continue::continuation();
        senders.push(tx);
        futures.push(rx);
    }

    thread::spawn(move || {
        logwise::info_sync!("Background thread started, will submit {count} tasks", count = senders.len());
        for (s, sender) in senders.drain(..).enumerate() {
            eprintln!("Submitting task {}/{}", s + 1, NUM_ITERATIONS);

            // Record time just before submission
            let start_time = Instant::now();

            let task_label = format!("submit_to_main_thread_benchmark_task_{s}");
            //app_window::application::submit_to_main_thread(task_label, move || {
                // Record time when closure starts executing
                let elapsed = start_time.elapsed();
                // Send the result back
                sender.send((s, elapsed));
                eprintln!("Task {}/{} finished", s + 1, NUM_ITERATIONS);
            //});

            // Wait a bit between submissions to get clean measurements
            //thread::sleep(Duration::from_millis(1));
        }
        logwise::info_sync!("Background thread finished submitting all {count} tasks", count = NUM_ITERATIONS);
    });

    // Collect results
    logwise::warn_sync!("Starting to collect {count} results...", count = futures.len());
    for (i, recv) in futures.into_iter().enumerate() {
        // logwise::info_sync!("Waiting for result {partial}/{total}", partial = i + 1, total = NUM_ITERATIONS);
        let r = recv.await;
        //eprintln!("Received result {}/{}: {:?}", i + 1, NUM_ITERATIONS, r); //this prevents reproduction apparently
        //logwise::info_sync!("Received result {partial}/{total}", partial = i + 1, total = NUM_ITERATIONS);
        //stats.add_sample(r.1);
    }
    logwise::warn_sync!("Finished collecting all results!");
}