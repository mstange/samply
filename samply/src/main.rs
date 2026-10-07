#[cfg(target_os = "macos")]
mod mac;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

mod cli;
mod cli_utils;
mod config;
mod import;
mod linux_shared;
mod name;
mod profile_json_preparse;
mod profile_store;
mod server;
mod shared;
mod symbols;

use std::ffi::OsStr;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fxprof_processed_profile::Profile;
use shared::ctrl_c::CtrlC;

#[cfg(any(target_os = "android", target_os = "linux"))]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;
#[cfg(target_os = "windows")]
use windows::profiler;

use config::{Config, ProfilesConfig};
use profile_json_preparse::parse_libinfo_map_from_profile_file;
use profile_store::{ProfileDir, ProfileStore, PROFILE_FILE_EXTENSION};
use server::{start_server, RunningServerInfo, ServerProps};
use shared::presymbolicate::get_presymbolicate_info;
use shared::prop_types::{ImportProps, SymbolProps};
use shared::save_profile::save_profile_to_file;
use symbols::create_symbol_manager_and_quota_manager;

#[tokio::main]
async fn main() {
    env_logger::init();

    use clap::{CommandFactory, Parser};
    let opt = cli::Opt::parse();
    let config_path = opt.config.as_deref();
    match opt.action {
        cli::Action::Load(load_args) => do_load_action(load_args, &load_config(config_path)).await,
        cli::Action::Completions(args) => {
            clap_complete::generate(
                args.shell,
                &mut cli::Opt::command(),
                "samply",
                &mut std::io::stdout(),
            );
        }
        cli::Action::Import(import_args) => {
            do_import_action(import_args, &load_config(config_path)).await
        }

        #[cfg(any(
            target_os = "android",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))]
        cli::Action::Record(record_args) => {
            do_record_action(record_args, &load_config(config_path)).await
        }

        // Windows-only: elevated helper, to run xperf as an administrator.
        //
        // We don't have a load_config() call here; in fact we shouldn't put one
        // here because doing so would create a config file if it's not there yet,
        // possibly as a different user.
        #[cfg(target_os = "windows")]
        cli::Action::RunElevatedHelper(args) => {
            windows::run_elevated_helper(&args.ipc_directory, args.output_path)
        }

        #[cfg(target_os = "macos")]
        cli::Action::Setup(cli::SetupArgs { yes }) => mac::codesign_setup::codesign_setup(yes),
    }
}

fn load_config(explicit_path: Option<&Path>) -> Config {
    config::load(explicit_path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    })
}

/// Picks the output path: the explicit `-o` path, or a new file in the
/// profile store. An explicit path bypasses the store entirely.
///
/// Returns the store directory if the path is in the store.
fn resolve_output_path(
    explicit_output: Option<PathBuf>,
    profiles_config: &ProfilesConfig,
    profile_name: &str,
) -> (PathBuf, Option<ProfileDir>) {
    if let Some(path) = explicit_output {
        return (path, None);
    }

    match ProfileDir::open(profiles_config) {
        Ok(dir) => {
            let path = dir.new_profile_path(profile_name, SystemTime::now());
            (path, Some(dir))
        }
        Err(e) => {
            eprintln!("Warning: {e}. Writing the profile to the current directory instead.");
            (
                PathBuf::from(format!("profile.{PROFILE_FILE_EXTENSION}")),
                None,
            )
        }
    }
}

/// Opens eviction for the profile store, if there is a store directory.
fn open_profile_store(
    dir: Option<ProfileDir>,
    profiles_config: &ProfilesConfig,
) -> Option<ProfileStore> {
    let dir = dir?;
    Some(ProfileStore::for_dir(&dir, profiles_config))
}

fn is_inside_dir(path: &Path, dir: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    path.starts_with(dir)
}

async fn do_load_action(load_args: cli::LoadArgs, config: &Config) {
    // If the file lives in the profile store, mark it as recently used so
    // that it isn't evicted soon.
    let is_in_store = config
        .profiles
        .resolved_dir()
        .is_some_and(|dir| is_inside_dir(&load_args.file, &dir));
    let profile_dir = if is_in_store {
        ProfileDir::open(&config.profiles).ok()
    } else {
        None
    };
    let store = open_profile_store(profile_dir, &config.profiles);
    if let Some(store) = &store {
        store.on_profile_accessed(&load_args.file);
        store.trigger_eviction();
    }

    let symbol_props = load_args.symbol_props(config.symbols.to_symbol_props());
    serve_profile(&load_args.file, load_args.server_props(), symbol_props).await;

    if let Some(store) = store {
        store.finish().await;
    }
}

async fn do_import_action(import_args: cli::ImportArgs, config: &Config) {
    let input_path = &import_args.file;
    let input_file = File::open(input_path).unwrap_or_else(|err| {
        eprintln!("Could not open file {input_path:?}: {err}");
        std::process::exit(1)
    });

    let symbol_props = import_args.symbol_props(config.symbols.to_symbol_props());
    let import_props = import_args.import_props(symbol_props.clone());
    let presymbolicate = import_props.profile_creation_props.presymbolicate;
    let profile_name = import_props
        .profile_creation_props
        .profile_name()
        .to_string();
    let (output_path, profile_dir) =
        resolve_output_path(import_args.output.clone(), &config.profiles, &profile_name);

    let mut profile = convert_file_to_profile(&input_file, input_path, import_props);

    if presymbolicate {
        eprintln!("Symbolicating...");
        let symbol_info = get_presymbolicate_info(&profile, symbol_props.clone()).await;
        profile = profile.make_symbolicated_profile(&symbol_info);
        profile.set_symbolicated(true);
    }

    save_profile_to_file(&profile, &output_path).expect("Couldn't write JSON");
    eprintln!("Saved profile to {}", output_path.display());
    let store = open_profile_store(profile_dir, &config.profiles);
    if let Some(store) = &store {
        store.on_profile_saved(&output_path);
    }

    // Drop the profile so that it doesn't take up memory while the server is running.
    drop(profile);

    if let Some(server_props) = import_args.server_props() {
        serve_profile(&output_path, server_props, symbol_props).await;
    }

    if let Some(store) = store {
        store.finish().await;
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
))]
async fn do_record_action(record_args: cli::RecordArgs, config: &Config) {
    let recording_mode = record_args.recording_mode();
    let profile_creation_props = record_args.profile_creation_props();
    let presymbolicate = profile_creation_props.presymbolicate;
    let symbol_props = record_args.symbol_props(config.symbols.to_symbol_props());

    // The output path must be known before recording starts: on Windows, the
    // ETL file paths are derived from it.
    let (output_path, profile_dir) = resolve_output_path(
        record_args.output.clone(),
        &config.profiles,
        profile_creation_props.profile_name(),
    );
    let recording_props = record_args.recording_props(output_path.clone());

    let (mut profile, exit_status) =
        profiler::run(recording_mode, recording_props, profile_creation_props).unwrap_or_else(
            |err| {
                eprintln!("Encountered an error during profiling: {err:?}");
                std::process::exit(1);
            },
        );

    if presymbolicate {
        eprintln!("Symbolicating...");
        let symbol_info = get_presymbolicate_info(&profile, symbol_props.clone()).await;
        profile = profile.make_symbolicated_profile(&symbol_info);
        profile.set_symbolicated(true);
    }

    save_profile_to_file(&profile, &output_path).expect("Couldn't write JSON");
    eprintln!("Saved profile to {}", output_path.display());
    let store = open_profile_store(profile_dir, &config.profiles);
    if let Some(store) = &store {
        store.on_profile_saved(&output_path);

        // Kept ETL files sit next to the profile; make them subject to eviction too.
        #[cfg(target_os = "windows")]
        if record_args.keep_etl {
            store.register_existing_file(&windows::etl_path_for_output(&output_path, "kernel.etl"));
            store.register_existing_file(&windows::etl_path_for_output(&output_path, "user.etl"));
        }
    }

    // Drop the profile so that it doesn't take up memory while the server is running.
    drop(profile);

    // then fire up the server for the profiler front end, if not save-only
    if let Some(server_props) = record_args.server_props() {
        serve_profile(&output_path, server_props, symbol_props).await;
    }

    if let Some(store) = store {
        store.finish().await;
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
    import::perf::convert(
        reader,
        file_mod_time,
        binary_lookup_dirs,
        aux_file_lookup_dirs,
        import_props.profile_creation_props,
    )
    .unwrap_or_else(|error| {
        eprintln!("Error importing perf.data file: {error:?}");
        std::process::exit(1);
    })
}

/// Serves the profile and its symbols until Ctrl+C is pressed.
async fn serve_profile(profile_path: &Path, server_props: ServerProps, symbol_props: SymbolProps) {
    let libinfo_map = {
        let profile_file = File::open(profile_path).unwrap_or_else(|err| {
            eprintln!("Could not open file {profile_path:?}: {err}");
            std::process::exit(1)
        });

        parse_libinfo_map_from_profile_file(profile_file, profile_path)
            .expect("Couldn't parse libinfo map from profile file")
    };

    let (mut symbol_manager, quota_manager) =
        create_symbol_manager_and_quota_manager(symbol_props, server_props.verbose);
    for lib_info in libinfo_map.into_values() {
        symbol_manager.add_known_library(lib_info);
    }

    let precog_path = profile_path.with_extension("syms.json");
    if let Some(precog_info) = shared::symbol_precog::PrecogSymbolInfo::try_load(&precog_path) {
        for symbol_map in precog_info.into_iter() {
            let lib_info = symbol_map.library_info();
            symbol_manager.add_known_library_symbols(lib_info, Arc::new(symbol_map));
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
}
