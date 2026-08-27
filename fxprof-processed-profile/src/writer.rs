//! JSON writer with optional JSLB output.

use std::io::Write;

use json_slabs::{Builder, SlabPrimitive, SLAB_REF_KEY};
use struson::writer::{FiniteNumber, JsonNumberError, JsonStreamWriter, JsonWriter};

/// All write_json methods in this crate take a `Writer`.
///
/// If `jslb_builder` is `None`, the writer just writes to the inner
/// `JsonStreamWriter`, with a few extra convenience methods around it.
///
/// If `jslb_builder` is `Some`, the `JsonStreamWriter` is the writer
/// for a JSON slab. This can be the root JSON slab, or it can be a
/// nested JSON object that was extracted into its own slab, see
/// `Writer::split_out_object`.
///
/// This writer will create typed-array slabs when methods like `typed_array`
/// or `typed_array_from_iter` are called - in JSON mode those just become
/// regular number arrays in the JSON.
///
/// Lifetimes:
/// - 'w: The mutable borrows of the JSON stream writer and the JSLB builder
/// - 'p: The profile-owned data referenced by the JSLB builder, e.g. columns in tables
pub struct Writer<'w, 'p, W: Write> {
    pub json: &'w mut JsonStreamWriter<W>,
    pub jslb_builder: Option<&'w mut Builder<'p>>,
}

impl<'p, W: Write> Writer<'_, 'p, W> {
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

    /// Write a numeric column. In JSLB mode, register the slice as a
    /// typed-array slab (bytes written directly at flush time on LE
    /// hosts) and emit a `{"$s":N}` placeholder. Otherwise, write the
    /// values inline as a JSON array of numbers.
    ///
    /// The Rust element type picks the slab type, and thus the kind of typed
    /// array the front-end will see: `i32` becomes an `Int32Array`, `u8` a
    /// `Uint8Array`, and so on.
    ///
    /// Warning: Only use this for columns which are allowed to be typed arrays
    /// in the version of the "processed profile" format that we emit! Otherwise,
    /// the front-end will have a typed array in a place where it was expecting
    /// a regular JS array, and things won't work correctly. This includes the
    /// element type: a column which is specified as an `Int32Array` must not be
    /// written as, say, a `u16` slice.
    pub fn typed_array<N: SlabPrimitive + FiniteNumber + Copy + 'p>(
        &mut self,
        values: &'p [N],
    ) -> std::io::Result<()> {
        match self.jslb_builder.as_deref_mut() {
            Some(builder) => {
                let p = builder.add_slab(values);
                self.write_slab_placeholder(p)
            }
            None => self.number_array(values),
        }
    }

    /// Write a numeric column produced by `iter`. In JSLB mode, register the
    /// iterator as a streaming typed-array slab (no intermediate `Vec<N>` is
    /// materialized); otherwise, walk the iterator and write the values inline.
    ///
    /// Use this for columns which aren't stored as a contiguous slice, e.g.
    /// constant columns. See [`Writer::typed_array`] for the caveats about
    /// which columns may be written as typed arrays, and about the element type.
    pub fn typed_array_from_iter<N, I>(&mut self, count: usize, iter: I) -> std::io::Result<()>
    where
        N: SlabPrimitive + FiniteNumber + Copy + 'p,
        I: IntoIterator<Item = N>,
        I::IntoIter: 'p,
    {
        match self.jslb_builder.as_deref_mut() {
            Some(builder) => {
                let p = builder.add_slab_from_iter(count, iter);
                self.write_slab_placeholder(p)
            }
            None => self.array(|w| {
                for v in iter {
                    w.json.number_value(v)?;
                }
                Ok(())
            }),
        }
    }

    /// Write an `f64` column produced by `iter`. In JSLB mode, register
    /// the iterator as a streaming typed-array slab (no intermediate
    /// `Vec<f64>` is materialized); otherwise, walk the iterator and
    /// write the values inline.
    ///
    /// Warning: Only use this for columns which are allowed to be typed arrays
    /// in the version of the "processed profile" format that we emit! Otherwise,
    /// the front-end will have a typed array in a place where it was expecting
    /// a regular JS array, and things won't work correctly.
    pub fn f64_array_from_iter<I>(&mut self, count: usize, iter: I) -> std::io::Result<()>
    where
        I: IntoIterator<Item = f64> + 'p,
        I::IntoIter: 'p,
    {
        match self.jslb_builder.as_deref_mut() {
            Some(builder) => {
                let p = builder.add_slab_from_iter(count, iter);
                self.write_slab_placeholder(p)
            }
            None => self.array(|w| {
                for v in iter {
                    w.fp(v)?;
                }
                Ok(())
            }),
        }
    }

    /// Write a JSON sub-document. In JSLB mode, run `body` into a scratch
    /// buffer, register the buffer as a `SlabType::Json` slab, and emit a
    /// `{"$s":N}` placeholder on the current stream. Otherwise, run `body`
    /// directly on the current writer.
    pub fn split_out_object<B: SplitOutObjectBody<'p>>(&mut self, body: B) -> std::io::Result<()> {
        let Some(builder) = self.jslb_builder.as_deref_mut() else {
            return body.write_body(self);
        };
        let mut buf = Vec::new();
        {
            let mut json = JsonStreamWriter::new(&mut buf);
            let mut inner = Writer {
                json: &mut json,
                jslb_builder: Some(&mut *builder),
            };
            body.write_body(&mut inner)?;
            json.finish_document()?;
        }
        let p = builder.add_json_slab(buf);
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
pub(crate) trait SplitOutObjectBody<'p> {
    fn write_body<W: Write>(self, w: &mut Writer<'_, 'p, W>) -> std::io::Result<()>;
}
