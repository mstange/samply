use std::io::Write;

use crate::writer::Writer;

/// One labeled value in a section of the profile's `meta.extra`, see
/// [`Profile::add_extra_info_section`](crate::Profile::add_extra_info_section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraInfoEntry {
    label: String,
    value: String,
}

impl ExtraInfoEntry {
    /// An entry whose value the Firefox Profiler shows as plain text.
    pub fn string(label: &str, value: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            w.name("label")?;
            w.string_value(&self.label)?;
            // The Firefox Profiler formats these values without a string
            // table, so string-index formats such as `unique-string` do not
            // work here. `string` carries the value itself.
            w.name("format")?;
            w.string_value("string")?;
            w.name("value")?;
            w.string_value(&self.value)
        })
    }
}

/// A labeled group of entries in `meta.extra`.
#[derive(Debug, Clone)]
pub(crate) struct ExtraInfoSection {
    pub(crate) label: String,
    pub(crate) entries: Vec<ExtraInfoEntry>,
}

impl ExtraInfoSection {
    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            w.name("label")?;
            w.string_value(&self.label)?;
            w.name("entries")?;
            w.array(|w| {
                for entry in &self.entries {
                    entry.write_json(w)?;
                }
                Ok(())
            })
        })
    }
}
