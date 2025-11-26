use rayon::{ThreadPoolBuilder, prelude::*};
use samply_markers::prelude::*;
use std::{collections::HashSet, time::Duration};

fn contains_digit(mut value: u32, digit: u32) -> bool {
    if value == 0 {
        return digit == 0;
    }

    while value > 0 {
        if value % 10 == digit {
            return true;
        }
        value /= 10;
    }

    false
}

fn filter_numbers_with_digit(
    chunk_id: usize,
    numbers: &[u32],
    digit: u32,
    parallel_filters: bool,
) -> Vec<u32> {
    let mode = if parallel_filters {
        "parallel"
    } else {
        "sequential"
    };

    samply_measure!({
        if parallel_filters {
            numbers
                .par_iter()
                .filter(|&&n| contains_digit(n, digit))
                .cloned()
                .collect()
        } else {
            numbers
                .iter()
                .filter(|&&n| contains_digit(n, digit))
                .cloned()
                .collect()
        }
    }, marker: {
        name: format!("chunk {chunk_id} filter digit {digit} ({mode})"),
    })
}

fn symmetric_difference_sum(chunk_id: usize, twos: &[u32], fives: &[u32]) -> u64 {
    samply_measure!({
        let two_set: HashSet<u32> = twos.iter().copied().collect();
        let five_set: HashSet<u32> = fives.iter().copied().collect();

        let only_twos: u64 = two_set
            .difference(&five_set)
            .map(|&value| value as u64)
            .sum();
        let only_fives: u64 = five_set
            .difference(&two_set)
            .map(|&value| value as u64)
            .sum();

        only_twos + only_fives
    }, marker: {
        name: format!("chunk {chunk_id} symmetric diff"),
    })
}

fn process_chunk_pipeline(chunk_id: usize, numbers: &[u32], parallel_filters: bool) -> u64 {
    let _chunk_timer = samply_timer!({ name: format!("chunk {chunk_id} pipeline") });

    let twos = filter_numbers_with_digit(chunk_id, numbers, 2, parallel_filters);
    let fives = filter_numbers_with_digit(chunk_id, numbers, 5, parallel_filters);

    symmetric_difference_sum(chunk_id, &twos, &fives)
}

fn generate_chunks(chunk_count: usize, chunk_len: usize) -> Vec<Vec<u32>> {
    let mut seed = 0u64;

    (0..chunk_count)
        .map(|_| {
            let mut values = Vec::with_capacity(chunk_len);
            for _ in 0..chunk_len {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                values.push((seed % 1_000_000) as u32);
            }
            values
        })
        .collect()
}

fn main() {
    let _main_timer = samply_timer!({ name: "main()" });

    ThreadPoolBuilder::new()
        .num_threads(8)
        .build_global()
        .expect("Failed to configure Rayon thread pool");

    println!("\n=== Digit Symmetric Difference Demo ===\n");

    let chunk_count = 32;
    let chunk_len = 131072;
    let chunks = generate_chunks(chunk_count, chunk_len);

    println!(
        "Generated {} chunks with {} numbers each ({} total)\n",
        chunk_count,
        chunk_len,
        chunk_count * chunk_len
    );

    println!("=== Sequential Processing ===");
    let sequential_results: Vec<_> = samply_measure!({
        chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| process_chunk_pipeline(i, chunk, false))
            .collect()
    }, marker: {
        name: "sequential pass",
    });
    let sequential_total: u64 = sequential_results.iter().sum();
    println!(
        "Sequential: {} chunks processed | total symmetric diff sum {}\n",
        sequential_results.len(),
        sequential_total
    );

    std::thread::sleep(Duration::from_millis(20));

    println!("=== Parallel Processing (Rayon) ===");
    let parallel_results: Vec<_> = samply_measure!({
        chunks
            .par_iter()
            .enumerate()
            .map(|(i, chunk)| process_chunk_pipeline(i + chunk_count, chunk, true))
            .collect()
    }, marker: {
        name: "parallel pass",
    });
    let parallel_total: u64 = parallel_results.iter().sum();
    println!(
        "Parallel: {} chunks processed | total symmetric diff sum {}\n",
        parallel_results.len(),
        parallel_total
    );
}
