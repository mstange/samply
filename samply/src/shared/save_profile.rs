use std::ffi::OsStr;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use flate2::{Compression, GzBuilder};
use fxprof_processed_profile::{Profile, ProfileFormat};

// Level two has an acceptable trade-off between how long compression
// takes and how much data it saves on the profile JSONs I tested with.
const GZIP_COMPRESSION_LEVEL: u32 = 2;

pub fn save_profile_to_file(profile: &Profile, output_path: &Path) -> std::io::Result<()> {
    let output_file = match File::create(output_path) {
        Ok(output_file) => output_file,
        Err(err) => {
            eprintln!("Couldn't create output file {output_path:?}: {err}");
            std::process::exit(1);
        }
    };

    let writer = BufWriter::new(output_file);
    let is_gz = output_path.extension() == Some(OsStr::new("gz"));
    // Route `.jslb` / `.jslb.gz` to JSLB output; anything else stays JSON.
    let uncompressed_stem: &Path = if is_gz {
        Path::new(output_path.file_stem().unwrap())
    } else {
        output_path
    };
    let format = if uncompressed_stem.extension() == Some(OsStr::new("jslb")) {
        ProfileFormat::JsonSlabs
    } else {
        ProfileFormat::Json
    };
    if is_gz {
        let name_without_gz = output_path.file_stem().unwrap().to_string_lossy();
        let builder = GzBuilder::new().filename(name_without_gz.as_bytes());
        let gz = builder.write(writer, Compression::new(GZIP_COMPRESSION_LEVEL));
        let gz = BufWriter::new(gz);
        profile.to_writer(gz, format)?;
    } else {
        profile.to_writer(writer, format)?;
    }
    Ok(())
}
