use std::io::Write;
use std::path::Path;

use base64::prelude::*;
use flate2::write::GzEncoder;
use flate2::Compression;

const PROFILER_API_URL: &str = "https://api.profiler.firefox.com/compressed-store";
const PROFILER_VIEW_URL: &str = "https://profiler.firefox.com/public/";

/// Uploads a profile to profiler.firefox.com and returns a shareable URL.
///
/// See https://github.com/firefox-devtools/profiler/blob/main/docs-developer/loading-in-profiles.md
pub fn upload_profile(profile_path: &Path) -> Result<String, ShareError> {
    let file_data = std::fs::read(profile_path).map_err(ShareError::ReadFile)?;

    let compressed_data = if is_gzipped(&file_data) {
        file_data
    } else {
        gzip_compress(&file_data)?
    };

    eprintln!("Uploading profile...");

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(PROFILER_API_URL)
        .header(
            "Accept",
            "application/vnd.firefox-profiler+json;version=1.0",
        )
        .body(compressed_data)
        .send()
        .map_err(ShareError::Upload)?;

    if !response.status().is_success() {
        return Err(ShareError::UploadFailed(response.status().to_string()));
    }

    let jwt = response.text().map_err(ShareError::ReadResponse)?;
    let token = decode_jwt_payload(&jwt)?;
    let url = format!("{}{}", PROFILER_VIEW_URL, token);

    Ok(url)
}

/// Returns `true` if the payload starts with gzip magic bytes 0x1f8b.
fn is_gzipped(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b
}

/// Compresses a payload with gzip.
fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, ShareError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).map_err(ShareError::Compress)?;
    encoder.finish().map_err(ShareError::Compress)
}

/// Decodes JWT payload returned from upload the profile to [`PROFILER_API_URL`].
///
/// Returns the profile token that can be used to view the profile with [`PROFILER_VIEW_URL`].
fn decode_jwt_payload(jwt: &str) -> Result<String, ShareError> {
    let parts: Vec<&str> = jwt.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(ShareError::InvalidJwt("expected 3 parts".to_string()));
    }

    let payload = parts[1];
    let decoded = BASE64_URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| BASE64_STANDARD.decode(payload))
        .map_err(|e| ShareError::InvalidJwt(format!("base64 decode failed: {e}")))?;

    let payload_str =
        String::from_utf8(decoded).map_err(|e| ShareError::InvalidJwt(format!("utf8: {e}")))?;

    let json: serde_json::Value = serde_json::from_str(&payload_str)
        .map_err(|e| ShareError::InvalidJwt(format!("json parse: {e}")))?;

    json.get("profileToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ShareError::InvalidJwt("missing profileToken field".to_string()))
}

#[derive(Debug)]
pub enum ShareError {
    ReadFile(std::io::Error),
    Compress(std::io::Error),
    Upload(reqwest::Error),
    UploadFailed(String),
    ReadResponse(reqwest::Error),
    InvalidJwt(String),
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::ReadFile(e) => write!(f, "failed to read profile file: {e}"),
            ShareError::Compress(e) => write!(f, "failed to compress profile: {e}"),
            ShareError::Upload(e) => write!(f, "failed to upload profile: {e}"),
            ShareError::UploadFailed(status) => {
                write!(f, "profile upload failed with status: {status}")
            }
            ShareError::ReadResponse(e) => write!(f, "failed to read upload response: {e}"),
            ShareError::InvalidJwt(msg) => write!(f, "invalid JWT response: {msg}"),
        }
    }
}
