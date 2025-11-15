use samply_markers::prelude::*;

async fn fetch_url(url: &str) -> (String, Option<String>) {
    // Emit an interval marker for the time it takes to fetch.
    let result = samply_measure!(async {
        let response = reqwest::get(url).await?;
        response.text().await
    }, marker: {
        name: format!("fetch {url}"),
    })
    .await;

    match result {
        Ok(content) => {
            println!("  ✓ Fetched {url} ({} bytes)", content.len());
            (String::from(url), Some(content))
        }
        Err(error) => {
            // Emit an instant marker any time a fetch fails.
            samply_marker!({ name: format!("fetch failed: {url}") });
            println!("  ✗ Failed to fetch {url}: {error}");
            (String::from(url), None)
        }
    }
}

#[tokio::main]
async fn main() {
    // Create a timer that will span the entirety of main().
    // The timer will emit an interval marker when it is dropped at the end of the scope.
    let _main_timer = samply_timer!({ name: "main()" });

    println!("\nStarting samply-markers demo...\n");

    std::thread::sleep(std::time::Duration::from_millis(200));

    let urls = &[
        "https://example.com",
        "https://fail.invalid",
        "https://fail.invalid",
        "https://en.wikipedia.org/wiki/Firefox",
        "https://fail.invalid",
        "https://github.com/nordzilla",
    ];

    println!("\n=== Sequential Fetching ===\n");

    // Emit an interval marker for the total time to fetch all URLs sequentially.
    samply_measure!({
        for url in urls {
            fetch_url(url).await;
        }
    }, marker: {
        name: "fetch all URLs sequentially",
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    println!("\n=== Concurrent Fetching ===\n");

    // Emit an interval marker for the total time to fetch all URLs concurrently.
    samply_measure!({
        futures::future::join_all(
            urls.iter().map(|url| fetch_url(url))
        ).await;
    }, marker: {
        name: "fetch all URLs concurrently",
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    println!("\nDemo completed!\n");
}
