#[cfg(target_os = "macos")]
mod mac;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

mod cli;
mod cli_utils;
mod import;
mod linux_shared;
mod name;
mod profile_json_preparse;
mod server;
mod shared;
mod symbols;

use std::ffi::OsStr;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use debugid::DebugId;
use fxprof_processed_profile::Profile;
use shared::ctrl_c::CtrlC;

#[cfg(any(target_os = "android", target_os = "linux"))]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;
use wholesym::{CodeId, LibraryInfo};
#[cfg(target_os = "windows")]
use windows::profiler;

use profile_json_preparse::parse_libinfo_map_from_profile_file;
use server::{start_server, RunningServerInfo, ServerProps};
use shared::prop_types::{ImportProps, SymbolProps};
use shared::save_profile::save_profile_to_file;
use symbols::create_symbol_manager_and_quota_manager;

fn main() {
    env_logger::init();

    use clap::Parser;
    let opt = cli::Opt::parse();
    match opt.action {
        cli::Action::Load(load_args) => do_load_action(load_args),
        cli::Action::Import(import_args) => do_import_action(import_args),

        #[cfg(any(
            target_os = "android",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))]
        cli::Action::Record(record_args) => do_record_action(record_args),

        #[cfg(target_os = "windows")]
        cli::Action::RunElevatedHelper(args) => {
            windows::run_elevated_helper(&args.ipc_directory, args.output_path)
        }

        #[cfg(target_os = "macos")]
        cli::Action::Setup(cli::SetupArgs { yes }) => mac::codesign_setup::codesign_setup(yes),
    }
}

fn do_load_action(load_args: cli::LoadArgs) {
    run_server_serving_profile(
        &load_args.file,
        load_args.server_props(),
        load_args.symbol_props(),
    );
}

fn do_import_action(import_args: cli::ImportArgs) {
    let input_path = &import_args.file;
    let input_file = match File::open(input_path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Could not open file {input_path:?}: {err}");
            std::process::exit(1)
        }
    };

    let import_props = import_args.import_props();
    let presymbolicate = import_props.profile_creation_props.presymbolicate;
    let mut profile = convert_file_to_profile(&input_file, input_path, import_props);

    if presymbolicate {
        eprintln!("Symbolicating...");
        let symbol_info = crate::shared::presymbolicate::get_presymbolicate_info(
            &profile,
            import_args.symbol_props(),
        );
        profile = profile.make_symbolicated_profile(&symbol_info);
        profile.set_symbolicated(true);
    }

    save_profile_to_file(&profile, &import_args.output).expect("Couldn't write JSON");

    // Drop the profile so that it doesn't take up memory while the server is running.
    drop(profile);

    if let Some(server_props) = import_args.server_props() {
        run_server_serving_profile(
            &import_args.output,
            server_props,
            import_args.symbol_props(),
        );
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
))]
fn do_record_action(record_args: cli::RecordArgs) {
    let recording_props = record_args.recording_props();
    let recording_mode = record_args.recording_mode();
    let profile_creation_props = record_args.profile_creation_props();
    let presymbolicate = profile_creation_props.presymbolicate;

    let (mut profile, exit_status) =
        match profiler::run(recording_mode, recording_props, profile_creation_props) {
            Ok(exit_status) => exit_status,
            Err(err) => {
                eprintln!("Encountered an error during profiling: {err:?}");
                std::process::exit(1);
            }
        };

    if presymbolicate {
        eprintln!("Symbolicating...");
        let symbol_info = crate::shared::presymbolicate::get_presymbolicate_info(
            &profile,
            record_args.symbol_props(),
        );
        profile = profile.make_symbolicated_profile(&symbol_info);
        profile.set_symbolicated(true);
    }

    save_profile_to_file(&profile, &record_args.output).expect("Couldn't write JSON");

    // Drop the profile so that it doesn't take up memory while the server is running.
    drop(profile);

    // then fire up the server for the profiler front end, if not save-only
    if let Some(server_props) = record_args.server_props() {
        run_server_serving_profile(
            &record_args.output,
            server_props,
            record_args.symbol_props(),
        );
    }

    std::process::exit(exit_status.code().unwrap_or(0));
}

fn convert_file_to_profile(
    input_file: &File,
    input_path: &Path,
    import_props: ImportProps,
) -> Profile {
    if input_path.extension() == Some(OsStr::new("etl")) {
        #[cfg(target_os = "windows")]
        {
            return windows::import::convert_etl_file_to_profile(input_path, import_props);
        }

        #[cfg(not(target_os = "windows"))]
        {
            eprintln!(
                "Error: Could not import ETW trace from file {}",
                input_path.to_string_lossy()
            );
            eprintln!("Importing ETW traces is only supported on Windows.");
            std::process::exit(1);
        }
    }

    // Treat all other files as perf.data files from Linux perf / Android simpleperf.

    let path = input_path
        .canonicalize()
        .expect("Couldn't form absolute path");
    let file_meta = input_file.metadata().ok();
    let file_mod_time = file_meta.and_then(|metadata| metadata.modified().ok());
    let mut binary_lookup_dirs = import_props.symbol_props.symbol_dir;
    let mut aux_file_lookup_dirs = import_props.aux_file_dir;
    if let Some(parent_dir) = path.parent() {
        binary_lookup_dirs.push(parent_dir.into());
        aux_file_lookup_dirs.push(parent_dir.into());
    }
    let reader = BufReader::new(input_file);
    match import::perf::convert(
        reader,
        file_mod_time,
        binary_lookup_dirs,
        aux_file_lookup_dirs,
        import_props.profile_creation_props,
    ) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("Error importing perf.data file: {error:?}");
            std::process::exit(1);
        }
    }
}

fn run_server_serving_profile(
    profile_path: &Path,
    server_props: ServerProps,
    symbol_props: SymbolProps,
) {
    let libinfo_map = {
        let profile_file = match File::open(profile_path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("Could not open file {profile_path:?}: {err}");
                std::process::exit(1)
            }
        };

        parse_libinfo_map_from_profile_file(profile_file, profile_path)
            .expect("Couldn't parse libinfo map from profile file")
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let (mut symbol_manager, quota_manager) =
            create_symbol_manager_and_quota_manager(symbol_props, server_props.verbose);
        for lib_info in libinfo_map.into_values() {
            symbol_manager.add_known_library(lib_info);
        }

        let precog_path = profile_path.with_extension("syms.json");
        if let Some(precog_info) = shared::symbol_precog::PrecogSymbolInfo::try_load(&precog_path) {
            for syms in precog_info.into_iter() {
                let lib_info = LibraryInfo {
                    debug_name: Some(syms.debug_name.clone()),
                    debug_id: Some(DebugId::from_str(&syms.debug_id).unwrap()),
                    code_id: CodeId::from_str(&syms.code_id).ok(),
                    ..LibraryInfo::default()
                };
                symbol_manager.add_known_library_symbols(lib_info, Arc::new(syms));
            }
        }

        let ctrl_c_receiver = CtrlC::observe_oneshot();

        let open_in_browser = server_props.open_in_browser;

        let RunningServerInfo {
            server_join_handle,
            server_origin,
            profiler_url,
        } = start_server(
            Some(profile_path),
            server_props,
            symbol_manager,
            ctrl_c_receiver,
        )
        .await;

        eprintln!("Local server listening at {server_origin}");
        if !open_in_browser {
            if let Some(profiler_url) = &profiler_url {
                println!("{profiler_url}");
            }
        }
        eprintln!("Press Ctrl+C to stop.");

        if open_in_browser {
            if let Some(profiler_url) = &profiler_url {
                let _ = opener::open_browser(profiler_url);
            }
        }

        // Run this server until it stops.
        if let Err(e) = server_join_handle.await {
            eprintln!("server error: {e}");
        }

        if let Some(quota_manager) = quota_manager {
            quota_manager.finish().await;
        }
    });
}
