use samply_markers::prelude::*;

fn fib(n: u64) -> u64 {
    samply_measure!({
        match n {
            0 | 1 => n,
            _ => fib(n - 1) + fib(n - 2),
        }
    }, marker: {
        name: format!("fib({n})"),
    })
}

fn main() {
    let n = 10;
    let value = fib(n);
    println!("fib({n}) = {}", value);
}
