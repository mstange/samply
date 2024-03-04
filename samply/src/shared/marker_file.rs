use fxprof_processed_profile::Timestamp;

use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::Path;

use super::timestamp_converter::TimestampConverter;
use super::utils::open_file_with_fallback;

#[derive(Debug, Clone)]
pub struct MarkerSpan {
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub name: String,
}

fn process_marker_span_line(
    line: &str,
    timestamp_converter: &TimestampConverter,
) -> Option<MarkerSpan> {
    let mut split = line.splitn(3, ' ');
    let start_time = split.next()?;
    let end_time = split.next()?;
    let name = split.next()?.to_owned();
    if name.is_empty() {
        return None;
    }
    let start_time = timestamp_converter.convert_time(start_time.parse::<u64>().ok()?);
    let end_time = timestamp_converter.convert_time(end_time.parse::<u64>().ok()?);
    Some(MarkerSpan {
        start_time,
        end_time,
        name,
    })
}

pub struct MarkerFile {
    lines: Lines<BufReader<File>>,
    timestamp_converter: TimestampConverter,
}

impl MarkerFile {
    pub fn parse(file: File, timestamp_converter: TimestampConverter) -> Self {
        Self {
            lines: BufReader::new(file).lines(),
            timestamp_converter,
        }
    }
}

impl Iterator for MarkerFile {
    type Item = MarkerSpan;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.lines.next()?.ok()?;
        process_marker_span_line(&line, &self.timestamp_converter)
    }
}

pub fn get_markers(
    marker_file: &Path,
    extra_dir: Option<&Path>,
    timestamp_converter: TimestampConverter,
) -> Result<Vec<MarkerSpan>, std::io::Error> {
    let (f, _true_path) = open_file_with_fallback(marker_file, extra_dir)?;
    let marker_file = MarkerFile::parse(f, timestamp_converter);
    let mut marker_spans: Vec<MarkerSpan> = marker_file.collect();
    marker_spans.sort_by_key(|m| m.start_time);
    Ok(marker_spans)
}
