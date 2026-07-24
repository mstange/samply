//! JSON writer with optional JSLB participation.
//!
//! `Writer` bundles the JSON stream writer with, optionally, a JSLB
//! [`Builder`] and its owned scratch-buffer arena. Every `write_json` method
//! in the crate takes `&mut Writer<W>` so that any level of the tree can
//! participate in JSLB output: numeric columns route through the builder as
//! typed-array slabs, and select sub-documents (`shared.frameTable`,
//! `shared.funcTable`, `shared.stringArray`, `threads`) get split out into
//! their own `SlabType::Json` slabs.
//!
//! For inline JSON output (`ProfileFormat::Json`), `jslb_builder` is `None`
//! and everything writes through the underlying `JsonStreamWriter`.
//!
//! Primitive JSON operations (`name`, `string_value`, `number_value`,
//! `bool_value`, `null_value`) are forwarded to `self.json`. Callers can
//! either use the forwarders on `Writer` or reach `self.json` directly.

use std::io::Write;

use elsa::FrozenVec;
use json_slabs::{Builder, JsonBytes, SLAB_REF_KEY};
use struson::writer::{FiniteNumber, JsonNumberError, JsonStreamWriter, JsonWriter};

pub struct Writer<'a, 'b: 'a, W: Write> {
    pub json: &'a mut JsonStreamWriter<W>,
    pub jslb_builder: Option<&'a mut Builder<'b>>,
    pub owned: &'b FrozenVec<Vec<u8>>,
}

impl<'b, W: Write> Writer<'_, 'b, W> {
    // -- Compound helpers ----------------------------------------------------

    #[inline]
    pub fn object<F>(&mut self, f: F) -> std::io::Result<()>
    where
        F: FnOnce(&mut Self) -> std::io::Result<()>,
    {
        self.json.begin_object()?;
        f(self)?;
        self.json.end_object()
    }

    #[inline]
    pub fn array<F>(&mut self, f: F) -> std::io::Result<()>
    where
        F: FnOnce(&mut Self) -> std::io::Result<()>,
    {
        self.json.begin_array()?;
        f(self)?;
        self.json.end_array()
    }

    #[inline]
    pub fn fp(&mut self, value: f64) -> std::io::Result<()> {
        self.json.fp_number_value(value).map_err(|e| match e {
            JsonNumberError::InvalidNumber(msg) => {
                std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
            }
            JsonNumberError::IoError(e) => e,
        })
    }

    #[inline]
    pub fn empty_array(&mut self) -> std::io::Result<()> {
        self.array(|_| Ok(()))
    }

    #[inline]
    pub fn null_array(&mut self, len: usize) -> std::io::Result<()> {
        self.array(|w| {
            for _ in 0..len {
                w.json.null_value()?;
            }
            Ok(())
        })
    }

    #[inline]
    pub fn number_array<N: FiniteNumber + Copy>(&mut self, values: &[N]) -> std::io::Result<()> {
        self.array(|w| {
            for &v in values {
                w.json.number_value(v)?;
            }
            Ok(())
        })
    }

    #[inline]
    pub fn optional_number_array<N: FiniteNumber + Copy>(
        &mut self,
        values: &[Option<N>],
    ) -> std::io::Result<()> {
        self.array(|w| {
            for v in values {
                match v {
                    Some(v) => w.json.number_value(*v)?,
                    None => w.json.null_value()?,
                }
            }
            Ok(())
        })
    }

    // -- Primitive forwarders -----------------------------------------------

    #[inline]
    pub fn name(&mut self, n: &str) -> std::io::Result<()> {
        self.json.name(n)
    }

    #[inline]
    pub fn string_value(&mut self, v: &str) -> std::io::Result<()> {
        self.json.string_value(v)
    }

    #[inline]
    pub fn number_value<N: FiniteNumber>(&mut self, v: N) -> std::io::Result<()> {
        self.json.number_value(v)
    }

    #[inline]
    pub fn bool_value(&mut self, v: bool) -> std::io::Result<()> {
        self.json.bool_value(v)
    }

    #[inline]
    pub fn null_value(&mut self) -> std::io::Result<()> {
        self.json.null_value()
    }

    // -- JSLB routing -------------------------------------------------------

    /// Write an `i32` column. In JSLB mode, register the slice as a borrowed
    /// typed-array slab and emit a `{"$s":N}` placeholder. Otherwise, write
    /// the values inline as a JSON array of numbers.
    ///
    /// Warning: Only use this for columns which are allowed to be typed arrays
    /// in the version of the "processed profile" format that we emit! Otherwise,
    /// the front-end will have a typed array in a place where it was expecting
    /// a regular JS array, and things won't work correctly.
    pub fn i32_array(&mut self, values: &'b [i32]) -> std::io::Result<()> {
        let p = match self.jslb_builder.as_deref_mut() {
            Some(builder) => builder.add_slab(values),
            None => return self.number_array(values),
        };
        self.write_slab_placeholder(p)
    }

    /// Write a JSON sub-document. In JSLB mode, run `body` into a scratch
    /// buffer, register the buffer as a `SlabType::Json` slab, and emit a
    /// `{"$s":N}` placeholder on the current stream. Otherwise, run `body`
    /// directly on the current writer.
    pub fn split_out_object<B: SplitOutObjectBody>(&mut self, body: B) -> std::io::Result<()> {
        let Some(builder) = self.jslb_builder.as_deref_mut() else {
            return body.write_body(self);
        };
        let owned = self.owned;
        let mut buf = Vec::new();
        {
            let mut json = JsonStreamWriter::new(&mut buf);
            let mut inner = Writer {
                json: &mut json,
                jslb_builder: Some(builder),
                owned,
            };
            body.write_body(&mut inner)?;
            json.finish_document()?;
        }
        let bytes: &'b [u8] = owned.push_get(buf);
        let p = builder.add_slab(JsonBytes(bytes));
        self.write_slab_placeholder(p)
    }

    fn write_slab_placeholder(&mut self, p: json_slabs::SlabPlaceholder) -> std::io::Result<()> {
        self.object(|w| {
            w.json.name(SLAB_REF_KEY)?;
            w.json.number_value(p.index())
        })
    }
}

/// A JSON sub-document that can be written either inline into the current
/// JSON stream or split out into a `SlabType::Json` JSLB slab.
///
/// The two paths use different underlying writer types (`JsonStreamWriter<W>`
/// for inline, `JsonStreamWriter<&mut Vec<u8>>` for the scratch buffer),
/// so the body must be generic over `W`. Rust's HRTBs don't quantify over
/// types, so a trait with a generic method is the way to express "this body
/// works for any `W: Write`".
pub(crate) trait SplitOutObjectBody {
    fn write_body<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()>;
}
