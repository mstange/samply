//! Thin JSON writer wrapper.
//!
//! `Writer` bundles the underlying `JsonStreamWriter` with a few convenience
//! helpers (`object`, `array`, `fp`, `empty_array`, `null_array`,
//! `number_array`, `optional_number_array`) that keep call sites in the rest
//! of the crate readable. Primitive JSON operations (`name`, `string_value`,
//! `number_value`, `bool_value`, `null_value`) are forwarded to `self.json`;
//! callers can also reach `self.json` directly.

use std::io::Write;

use struson::writer::{FiniteNumber, JsonNumberError, JsonStreamWriter, JsonWriter};

pub struct Writer<'a, W: Write> {
    pub json: &'a mut JsonStreamWriter<W>,
}

impl<W: Write> Writer<'_, W> {
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
}
