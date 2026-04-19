use std::pin::Pin;

use futures_util::{AsyncRead, TryStreamExt};

use crate::async_gzip_decoder::AsyncGzipDecoder;
use reqwest::header::{AsHeaderName, HeaderMap, CONTENT_ENCODING, CONTENT_LENGTH};

fn get_header<K: AsHeaderName>(headers: &HeaderMap, name: K) -> Option<String> {
    Some(headers.get(name)?.to_str().ok()?.to_ascii_lowercase())
}

enum TotalSize {
    Compressed(u64),
    Uncompressed(u64),
}

fn get_total_size(headers: &HeaderMap) -> Option<TotalSize> {
    let response_encoding = get_header(headers, CONTENT_ENCODING);
    let content_length =
        get_header(headers, CONTENT_LENGTH).and_then(|value| value.parse::<u64>().ok());

    // If the server sends a Content-Length header, use the size from that header.
    match content_length {
        Some(len) if len > 0 => {
            let total_size = match response_encoding.as_deref() {
                None => TotalSize::Uncompressed(len),
                Some(_) => TotalSize::Compressed(len),
            };
            return Some(total_size);
        }
        _ => {}
    }

    // Add a fallback for Google Cloud servers which use Transfer-Encoding: chunked with
    // HTTP/1.1 and thus do not include a Content-Length header.
    // This is the case for https://chromium-browser-symsrv.commondatastorage.googleapis.com/
    // (the Chrome symbol server) as of February 2023.
    if response_encoding.as_deref() == Some("gzip") {
        if let (Some("gzip"), Some(len)) = (
            get_header(headers, "x-goog-stored-content-encoding").as_deref(),
            get_header(headers, "x-goog-stored-content-length")
                .and_then(|value| value.parse::<u64>().ok()),
        ) {
            return Some(TotalSize::Compressed(len));
        }
    }

    // Add another fallback for AWS servers. I have not seen a case where this is necessary,
    // but it doesn't hurt either.
    if let Some(len) =
        get_header(headers, "x-amz-meta-original_size").and_then(|value| value.parse::<u64>().ok())
    {
        return Some(TotalSize::Uncompressed(len));
    }

    None
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Unexpected Content-Encoding header: {0}")]
    UnexpectedContentEncoding(String),
}

pub struct UncompressedStream {
    inner: UncompressedStreamInner,
    total_compressed_size: Option<u64>,
    total_uncompressed_size: Option<u64>,
    consumed_compressed_bytes: u64,
    produced_uncompressed_bytes: u64,
    progress_callback: Box<dyn FnMut(u64, Option<u64>) + Send + Sync + 'static>,
}

#[allow(clippy::large_enum_variant)]
enum UncompressedStreamInner {
    Decoder(AsyncGzipDecoder<Pin<Box<dyn AsyncRead + Send + Sync>>>),
    Stream(Pin<Box<dyn AsyncRead + Send + Sync>>),
}

impl UncompressedStream {
    fn new(
        inner: UncompressedStreamInner,
        total_size: Option<TotalSize>,
        progress_callback: Box<dyn FnMut(u64, Option<u64>) + Send + Sync + 'static>,
    ) -> Self {
        let (total_compressed_size, total_uncompressed_size) = match total_size {
            Some(TotalSize::Compressed(s)) => (Some(s), None),
            Some(TotalSize::Uncompressed(s)) => (None, Some(s)),
            None => (None, None),
        };
        Self {
            inner,
            total_compressed_size,
            total_uncompressed_size,
            consumed_compressed_bytes: 0,
            produced_uncompressed_bytes: 0,
            progress_callback,
        }
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use futures_util::AsyncReadExt;
        let (consumed_delta, produced) = match &mut self.inner {
            UncompressedStreamInner::Decoder(decoder) => {
                let prior_in = decoder.total_in();
                let produced = decoder.read(buf).await?;
                (decoder.total_in() - prior_in, produced)
            }
            UncompressedStreamInner::Stream(stream) => {
                let produced = stream.read(buf).await?;
                (produced as u64, produced)
            }
        };
        self.consumed_compressed_bytes += consumed_delta;
        self.produced_uncompressed_bytes += produced as u64;
        if self.total_compressed_size.is_some() || self.total_uncompressed_size.is_none() {
            (*self.progress_callback)(self.consumed_compressed_bytes, self.total_compressed_size);
        } else {
            (*self.progress_callback)(
                self.produced_uncompressed_bytes,
                self.total_uncompressed_size,
            );
        }
        Ok(produced)
    }
}

pub fn response_to_uncompressed_stream_with_progress<F>(
    response: reqwest::Response,
    progress: F,
) -> Result<UncompressedStream, Error>
where
    F: FnMut(u64, Option<u64>) + Send + Sync + 'static,
{
    let headers = response.headers();
    let response_encoding = get_header(headers, CONTENT_ENCODING);
    let total_size = get_total_size(headers);

    let stream = response.bytes_stream();
    let async_read = stream.map_err(std::io::Error::other).into_async_read();
    let async_read: Pin<Box<dyn AsyncRead + Send + Sync>> = Box::pin(async_read);

    let inner = match response_encoding.as_deref() {
        Some("gzip") => UncompressedStreamInner::Decoder(AsyncGzipDecoder::new(async_read)),
        Some(other_encoding) => {
            // Did we send a wrong Accept-Encoding header in the request? We only support gzip.
            return Err(Error::UnexpectedContentEncoding(other_encoding.to_string()));
        }
        None => UncompressedStreamInner::Stream(async_read),
    };
    Ok(UncompressedStream::new(
        inner,
        total_size,
        Box::new(progress),
    ))
}
