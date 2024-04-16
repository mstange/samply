extern crate futures;
extern crate tokio;

use wholesym::{SymbolManager, SymbolManagerConfig, FramesLookupResult};
use std::path::Path;
use futures::executor::block_on;
use std::env;
use tokio::{runtime,sync::mpsc::{channel, Receiver}};

fn parse_int(s: &str) -> std::result::Result<u64, std::num::ParseIntError> {
    if let Some(s) = s.strip_prefix("0x") {
        u64::from_str_radix(s, 16)
    } else if let Some(s) = s.strip_prefix("0o") {
        u64::from_str_radix(s, 8)
    } else if let Some(s) = s.strip_prefix("0b") {
        u64::from_str_radix(s, 2)
    } else {
        u64::from_str_radix(s, 10)
    }
}

async fn run(path: &str, addr: u64) -> Result<(), wholesym::Error> {
    let symbol_manager = SymbolManager::with_config(SymbolManagerConfig::new().respect_nt_symbol_path(true));
    let symbol_map = symbol_manager
        .load_symbol_map_for_binary_at_path(Path::new(path), None)
        .await?;
    println!("Looking up 0x{:x} in {}. Results:", addr, path);
    if let Some(address_info) = symbol_map.lookup_relative_address(addr.try_into().unwrap()) {
        println!(
            "Symbol: {:#x} {}",
            address_info.symbol.address, address_info.symbol.name
        );
        let frames = match address_info.frames {
            FramesLookupResult::Available(frames) => Some(frames),
            FramesLookupResult::External(ext_ref) => {
                symbol_manager
                    .lookup_external(&symbol_map.symbol_file_origin(), &ext_ref)
                    .await
            }
            FramesLookupResult::Unavailable => None,
        };
        if let Some(frames) = frames {
            for (i, frame) in frames.into_iter().enumerate() {
                let function = frame.function.unwrap();
                let file = frame.file_path.unwrap().display_path();
                let line = frame.line_number.unwrap();
                println!("  #{i:02} {function} at {file}:{line}");
            }
        }
    } else {
        println!("No symbol was found.");
    }
    Ok(())
}

fn main() {
    let (num_tokio_worker_threads, max_tokio_blocking_threads) = (4, 512); // 512 is tokio's current default
    println!("{}", num_tokio_worker_threads);
    let rt = runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .worker_threads(num_tokio_worker_threads)
        .max_blocking_threads(max_tokio_blocking_threads)
        .build().unwrap();


    let args: Vec<String> = env::args().collect();
    rt.block_on(async move {
        run(&args[1], parse_int(&args[2]).unwrap()).await;
    });
}
