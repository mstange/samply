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

use fxprof_processed_profile::Profile;

#[cfg(any(target_os = "android", target_os = "linux"))]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;
#[cfg(target_os = "windows")]
use windows::profiler;

use profile_json_preparse::parse_libinfo_map_from_profile_file;
use server::start_server_main;
use shared::prop_types::ImportProps;
use shared::save_profile::save_profile_to_file;

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
    let profile_filename = &load_args.file;
    let input_file = match File::open(profile_filename) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Could not open file {:?}: {}", profile_filename, err);
            std::process::exit(1)
        }
    };

    let libinfo_map = match parse_libinfo_map_from_profile_file(input_file, profile_filename) {
        Ok(libinfo_map) => libinfo_map,
        Err(err) => {
            eprintln!("Could not parse the input file as JSON: {}", err);
            eprintln!("If this is a perf.data file, please use `samply import` instead.");
            std::process::exit(1)
        }
    };
    start_server_main(
        profile_filename,
        load_args.server_props(),
        load_args.symbol_props(),
        libinfo_map,
    );
}

fn do_import_action(import_args: cli::ImportArgs) {
    let input_path = &import_args.file;
    let input_file = match File::open(input_path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Could not open file {:?}: {}", input_path, err);
            std::process::exit(1)
        }
    };

    let import_props = import_args.import_props();
    let unstable_presymbolicate = import_props.profile_creation_props.unstable_presymbolicate;
    let profile = convert_file_to_profile(&input_file, input_path, import_props);

    save_profile_to_file(&profile, &import_args.output).expect("Couldn't write JSON");

    if unstable_presymbolicate {
        crate::shared::symbol_precog::presymbolicate(
            &profile,
            &import_args.output.with_extension("syms.json"),
        );
    }

    if let Some(server_props) = import_args.server_props() {
        let profile_filename = &import_args.output;
        let libinfo_map = profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(profile_filename).expect("Couldn't open file we just wrote"),
            profile_filename,
        )
        .expect("Couldn't parse libinfo map from profile file");
        start_server_main(
            profile_filename,
            server_props,
            import_args.symbol_props(),
            libinfo_map,
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
    let unstable_presymbolicate = profile_creation_props.unstable_presymbolicate;

    let (profile, exit_status) =
        match profiler::run(recording_mode, recording_props, profile_creation_props) {
            Ok(exit_status) => exit_status,
            Err(err) => {
                eprintln!("Encountered an error during profiling: {err:?}");
                std::process::exit(1);
            }
        };

    save_profile_to_file(&profile, &record_args.output).expect("Couldn't write JSON");

    if unstable_presymbolicate {
        crate::shared::symbol_precog::presymbolicate(
            &profile,
            &record_args.output.with_extension("syms.json"),
        );
    }

    // then fire up the server for the profiler front end, if not save-only
    if let Some(server_props) = record_args.server_props() {
        let profile_filename = &record_args.output;
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(profile_filename).expect("Couldn't open file we just wrote"),
            profile_filename,
        )
        .expect("Couldn't parse libinfo map from profile file");
        start_server_main(
            profile_filename,
            server_props,
            record_args.symbol_props(),
            libinfo_map,
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
            eprintln!("Error importing perf.data file: {:?}", error);
            std::process::exit(1);
        }
    }
}
