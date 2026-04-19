use std::pin::Pin;
use std::task::Poll;

use futures_util::AsyncRead;
use zlib_rs::{Inflate, InflateFlush, Status};

/// Streaming gzip decoder that wraps any `AsyncRead` source.
pub struct AsyncGzipDecoder<R> {
    inner: R,
    inflate: Inflate,
    /// Staging buffer for compressed bytes read from `inner`.
    in_buf: Box<[u8]>,
    in_start: usize,
    in_end: usize,
    done: bool,
}

fn make_gzip_inflate() -> Inflate {
    // zlib-rs can decode both zlib and gzip wrappers, but the
    // documentation currently only mentions zlib support.
    // https://github.com/trifectatechfoundation/zlib-rs/issues/502
    //
    // To opt into gzip mode, we pass `zlib_header = true`` and
    // `window_bits = 15 + 16``.
    //
    // 15 = max window bits for gzip (32KiB), +16 selects the gzip wrapper.
    //
    // Passing a window_bits outside of `8..=15` is, strictly speaking,
    // an API contract violation. But maybe the comments will be adjusted
    // to officially allow this use.
    Inflate::new(true, 15 + 16)
}

impl<R: AsyncRead + Unpin> AsyncGzipDecoder<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            inflate: make_gzip_inflate(),
            in_buf: vec![0u8; 16 * 1024].into_boxed_slice(),
            in_start: 0,
            in_end: 0,
            done: false,
        }
    }

    /// Total number of compressed bytes consumed from the inner stream so far.
    pub fn total_in(&self) -> u64 {
        self.inflate.total_in()
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncGzipDecoder<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let me = self.get_mut();
        if me.done || buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            // Refill the staging buffer from the inner stream when exhausted.
            if me.in_start >= me.in_end {
                // Update in_start/in_end only after a successful read. If the
                // inner reader returns Pending, leave them unchanged so the next
                // poll sees the buffer as still exhausted and retries the refill.
                let n = match Pin::new(&mut me.inner).poll_read(cx, &mut me.in_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(n)) => n,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                };
                me.in_start = 0;
                me.in_end = n;
                if n == 0 {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "truncated gzip stream",
                    )));
                }
            }

            let input = &me.in_buf[me.in_start..me.in_end];
            let prior_in = me.inflate.total_in();
            let prior_out = me.inflate.total_out();
            let status = match me.inflate.decompress(input, buf, InflateFlush::NoFlush) {
                Ok(s) => s,
                Err(e) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        e.as_str(),
                    )));
                }
            };
            let consumed = (me.inflate.total_in() - prior_in) as usize;
            let produced = (me.inflate.total_out() - prior_out) as usize;
            me.in_start += consumed;

            if status == Status::StreamEnd {
                me.done = true;
                return Poll::Ready(Ok(produced));
            }
            if produced > 0 {
                return Poll::Ready(Ok(produced));
            }
        }
    }
}
