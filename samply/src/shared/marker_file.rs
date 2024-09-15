use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fxprof_processed_profile::Timestamp;

use super::timestamp_converter::TimestampConverter;
use super::utils::open_file_with_fallback;

#[derive(Debug, Clone)]
pub struct TracingTimings {
    pub time_busy: Duration,
    pub time_idle: Duration,
}

#[derive(Debug, Clone)]
pub struct EventOrSpanMarker {
    pub start_time: Timestamp,
    pub message: String,
    pub target: String,
    pub marker_data: MarkerData,
}

#[derive(Debug, Clone)]
pub enum MarkerData {
    Span(MarkerSpan),
    Event,
}

#[derive(Debug, Clone)]
pub struct MarkerSpan {
    pub end_time: Timestamp,
    pub label: String,
    pub action: String,
    pub timings: TracingTimings,
}

struct SpanTracker {
    started_span_cache: HashMap<u64, serde_json::Value>,
    start_keyword: String,
    end_keyword: String,
}

impl SpanTracker {
    fn new(start_keyword: &str, end_keyword: &str) -> Self {
        Self {
            start_keyword: start_keyword.to_string(),
            end_keyword: end_keyword.to_string(),
            started_span_cache: HashMap::new(),
        }
    }

    fn process_line(
        &mut self,
        id: u64,
        json: serde_json::Value,
    ) -> Option<(serde_json::Value, serde_json::Value)> {
        let message = json.get("fields")?.get("message")?.as_str()?.to_string();

        if message != self.start_keyword && message != self.end_keyword {
            return None;
        }

        if self.started_span_cache.contains_key(&id) {
            assert_eq!(message, self.end_keyword);
            let start = self.started_span_cache.remove(&id)?;
            Some((start, json))
        } else {
            assert_eq!(message, self.start_keyword);
            self.started_span_cache.insert(id, json);
            None
        }
    }
}

pub struct MarkerFile {
    lines: Lines<BufReader<File>>,
    timestamp_converter: TimestampConverter,
    new_close_tracker: SpanTracker,
    enter_exit_tracker: SpanTracker,
}

impl MarkerFile {
    pub fn parse(file: File, timestamp_converter: TimestampConverter) -> Self {
        Self {
            lines: BufReader::new(file).lines(),
            timestamp_converter,
            new_close_tracker: SpanTracker::new("new", "close"),
            enter_exit_tracker: SpanTracker::new("enter", "exit"),
        }
    }
}

fn parse_timing_field(fields: &serde_json::Value, field: &str) -> Option<Duration> {
    let field_str = fields.get(field)?.as_str().unwrap().replace("µ", "u");

    let end_idx = field_str
        .rfind(|c| char::is_numeric(c) || c == '.')
        .unwrap();
    let (num, unit) = field_str.split_at(end_idx + 1);

    Some(match unit {
        "s" => Duration::from_secs_f64(num.parse().unwrap()),
        "ms" => Duration::from_secs_f64(num.parse::<f64>().unwrap() / 1_000.0),
        "us" => Duration::from_secs_f64(num.parse::<f64>().unwrap() / 1_000_000.0),
        "ns" => Duration::from_secs_f64(num.parse::<f64>().unwrap() / 1_000_000_000.0),
        _ => panic!("unknown unit '{}' in field {}", unit, field_str),
    })
}

impl MarkerFile {
    fn read_timestamp_from_event(&self, json: &serde_json::Value) -> u64 {
        json.get("timestamp")
            .unwrap()
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    }

    fn process_complete_span(
        &mut self,
        start: serde_json::Value,
        end: serde_json::Value,
        label: &str,
    ) -> Option<EventOrSpanMarker> {
        let fields = end.get("fields").unwrap();

        let start_time = self.read_timestamp_from_event(&start);
        let end_time = self.read_timestamp_from_event(&end);

        let span = end.get("span").unwrap();

        let message = span
            .get("name")
            .map(|a| a.as_str().unwrap().to_string())
            .unwrap();
        let action = span
            .get("action")
            .map(|a| a.as_str().unwrap().to_string())
            .unwrap_or("No Action".to_string());
        let target = end.get("target").unwrap().as_str().unwrap().to_string();

        let time_busy = parse_timing_field(&fields, "time.busy")
            .unwrap_or(Duration::from_nanos(end_time - start_time));
        let time_idle = parse_timing_field(&fields, "time.idle").unwrap_or_default();

        Some(EventOrSpanMarker {
            start_time: self.timestamp_converter.convert_time(start_time),
            message,
            target,
            marker_data: MarkerData::Span(MarkerSpan {
                end_time: self.timestamp_converter.convert_time(end_time),
                action,
                label: label.to_string(),
                timings: TracingTimings {
                    time_busy: time_busy,
                    time_idle: time_idle,
                },
            }),
        })
    }

    fn process_event(&mut self, event: serde_json::Value) -> Option<EventOrSpanMarker> {
        let message = event
            .get("fields")
            .unwrap()
            .get("message")?
            .as_str()
            .unwrap()
            .to_string();
        let start_time = self
            .timestamp_converter
            .convert_time(self.read_timestamp_from_event(&event));
        let target = event.get("target").unwrap().as_str().unwrap().to_string();

        Some(EventOrSpanMarker {
            start_time,
            message,
            target,
            marker_data: MarkerData::Event,
        })
    }

    fn process_line(&mut self, line: &str) -> Option<EventOrSpanMarker> {
        let (id, json) = line.split_once(' ')?;
        let id = id.parse::<u64>().ok()?;
        let json: serde_json::Value = serde_json::from_str(json).ok()?;

        if id != 0 {
            if let Some((start, end)) = self.new_close_tracker.process_line(id, json.clone()) {
                self.process_complete_span(start, end, "Total")
            } else if let Some((start, end)) = self.enter_exit_tracker.process_line(id, json) {
                self.process_complete_span(start, end, "Running")
            } else {
                None
            }
        } else {
            self.process_event(json)
        }
    }
}

impl Iterator for MarkerFile {
    type Item = EventOrSpanMarker;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(line) = self.lines.next()?.ok() {
            if let Some(marker) = self.process_line(&line) {
                return Some(marker);
            }
        }
        None
    }
}

pub struct MarkerFileInfo {
    #[allow(dead_code)]
    pub prefix: String,
    #[allow(dead_code)]
    pub pid: u32,
    #[allow(dead_code)]
    pub tid: Option<u32>,
}

#[allow(unused)]
pub fn parse_marker_file_path(path: &Path) -> MarkerFileInfo {
    let filename = path.file_name().unwrap().to_str().unwrap();
    // strip .txt extension
    let filename = &filename[..filename.len() - 4];
    let mut parts = filename.splitn(3, '-');
    let prefix = parts.next().unwrap().to_owned();
    let pid = parts.next().unwrap().parse().unwrap();
    let tid = parts.next().map(|tid| tid.parse().unwrap());
    MarkerFileInfo { prefix, pid, tid }
}

pub fn get_markers(
    marker_file: &Path,
    lookup_dirs: &[PathBuf],
    timestamp_converter: TimestampConverter,
) -> Result<Vec<EventOrSpanMarker>, std::io::Error> {
    let (f, _true_path) = open_file_with_fallback(marker_file, lookup_dirs)?;
    let marker_file = MarkerFile::parse(f, timestamp_converter);
    let mut marker_spans: Vec<EventOrSpanMarker> = marker_file.collect();
    marker_spans.sort_by_key(|m| m.start_time);
    Ok(marker_spans)
}
