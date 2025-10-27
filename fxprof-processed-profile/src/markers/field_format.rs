use serde_derive::Serialize;

/// The field format for marker fields of kind [`MarkerFieldKind::String`](super::types::MarkerFieldKind::String).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerStringFieldFormat {
    // ----------------------------------------------------
    // String types.
    /// A URL, supports PII sanitization
    Url,

    /// A file path, supports PII sanitization.
    FilePath,

    /// A regular string, supports PII sanitization.
    /// Concretely this means that these strings are stripped when uploading
    /// profiles if you uncheck "Include resource URLs and paths".
    SanitizedString,

    /// A plain String, never sanitized for PII.
    ///
    /// Important: Do not put URL or file path information here, as it will not
    /// be sanitized during profile upload. Please be careful with including
    /// other types of PII here as well.
    #[serde(rename = "unique-string")]
    String,
}

/// The field format for marker fields of kind [`MarkerFieldKind::Number`](super::types::MarkerFieldKind::Number).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerNumberFieldFormat {
    // ----------------------------------------------------
    // Numeric types
    /// For time data that represents a duration of time.
    /// The value is given in float milliseconds and will be displayed
    /// in a unit that is picked based on the magnitude of the number.
    /// e.g. "Label: 5s, 5ms, 5μs"
    Duration,

    /// A timestamp, relative to the start of the profile. The value is given in
    /// float milliseconds.
    ///
    ///  e.g. "Label: 15.5s, 20.5ms, 30.5μs"
    Time,

    /// Display a millisecond value as seconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5s" for a value of 5000.0
    Seconds,

    /// Display a millisecond value as milliseconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5ms" for a value of 5.0
    Milliseconds,

    /// Display a millisecond value as microseconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5μs" for a value of 0.0005
    Microseconds,

    /// Display a millisecond value as seconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5ns" for a value of 0.0000005
    Nanoseconds,

    /// Display a bytes value in a unit that's appropriate for the number's magnitude.
    ///
    /// e.g. "Label: 5.55mb, 5 bytes, 312.5kb"
    Bytes,

    /// This should be a value between 0 and 1.
    /// e.g. "Label: 50%" for a value of 0.5
    Percentage,

    /// A generic integer number.
    /// Do not use it for time information.
    ///
    /// "Label: 52, 5,323, 1,234,567"
    Integer,

    /// A generic floating point number.
    /// Do not use it for time information.
    ///
    /// "Label: 52.23, 0.0054, 123,456.78"
    Decimal,
}

/// The field format for marker fields of kind [`MarkerFieldKind::Flow`](super::types::MarkerFieldKind::Flow).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerFlowFieldFormat {
    /// A flow is a u64 identifier that's unique across processes. All of
    /// the markers with same flow id before a terminating flow id will be
    /// considered part of the same "flow" and linked together.
    #[serde(rename = "flow-id")]
    Flow,

    /// A terminating flow ends a flow of a particular id and allows that id
    /// to be reused again. It often makes sense for destructors to create
    /// a marker with a field of this type.
    #[serde(rename = "terminating-flow-id")]
    TerminatingFlow,
}
