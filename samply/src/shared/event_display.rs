use clap::ValueEnum;
use rustc_hash::FxHashMap;
use std::{fmt::Display, str::FromStr};

use crate::shared::utils::glob_like_match;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum EventDisplay {
    #[default]
    Marker,
    Track,
    TimingData,
    RetainedAllocations,
    Allocations,
    Deallocations,
    Counter,
}
impl EventDisplay {
    fn capacity(&self) -> usize {
        match self {
            EventDisplay::Marker | EventDisplay::Track | EventDisplay::Counter => usize::MAX,
            EventDisplay::TimingData
            | EventDisplay::RetainedAllocations
            | EventDisplay::Allocations
            | EventDisplay::Deallocations => 1,
        }
    }
}
impl Display for EventDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.to_possible_value().unwrap().get_name())
    }
}
impl FromStr for EventDisplay {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        <EventDisplay as ValueEnum>::from_str(s, true)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventSelector {
    // number prefixed by #
    Position(usize),
    // Any string that does not contain *, stored as bytes for faster comparison
    Exact(Vec<u8>),
    // Stored in a vec of raw string bytes, with the first and last items
    // being the prefix/suffix, which need to be empty when the first/last character in the pattern is *.
    // Example: *foo -> vec![vec![], "foo".as_bytes()]
    Glob(Vec<Vec<u8>>),
}
impl Display for EventSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventSelector::Position(i) => write!(f, "Selector: #{}", i),
            EventSelector::Exact(items) => {
                write!(f, "Selector: {}", str::from_utf8(items).unwrap())
            }
            EventSelector::Glob(items) => {
                write!(
                    f,
                    "Selector: {}",
                    str::from_utf8(&items.join("*".as_bytes())).unwrap()
                )
            }
        }
    }
}
impl FromStr for EventSelector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let selector = if s.starts_with('#') {
            EventSelector::Position(
                s.split_at(1)
                    .1
                    .parse::<usize>()
                    .map_err(|e| e.to_string())?,
            )
        } else if s.contains('*') {
            EventSelector::Glob(s.split('*').map(|pat| pat.as_bytes().to_vec()).collect())
        } else {
            EventSelector::Exact(s.as_bytes().to_vec())
        };
        Ok(selector)
    }
}

impl EventSelector {
    pub fn matches(&self, i: usize, event: impl AsRef<str>) -> bool {
        match self {
            EventSelector::Position(pos) => i == *pos,
            EventSelector::Exact(items) => event.as_ref().as_bytes() == items,
            EventSelector::Glob(pattern) => glob_like_match(pattern, &event.as_ref().as_bytes()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventDisplaySelector {
    pub selector: EventSelector,
    pub display: EventDisplay,
}

impl Display for EventDisplaySelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.selector, self.display)
    }
}

impl FromStr for EventDisplaySelector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // selector could contain = , so we need to only split on the last one
        // possible selector characters checked with: `perf list --json | jq '.[].EventName' | grep -o . | sort -u`
        let (selector, action) = s
            .rsplit_once('=')
            .ok_or(format!("could not split on = in {s}"))?;

        Ok(EventDisplaySelector {
            selector: selector.parse()?,
            display: action.parse()?,
        })
    }
}

pub fn validate_events_display(event_display: &Vec<EventDisplaySelector>) -> Result<(), String> {
    let mut count = FxHashMap::default();
    for EventDisplaySelector { selector, display } in event_display {
        count
            .entry(display)
            .and_modify(|c: &mut usize| {
                *c += match selector {
                    EventSelector::Position(_) => 1,
                    EventSelector::Exact(_) => 1,
                    EventSelector::Glob(_) => 2, // could match multiple
                };
            })
            .or_default();
    }
    for (display, count) in count.iter() {
        if *count > display.capacity() {
            return Err(format!("Event display {display} can only show {} different events, but at least {count} events could match", display.capacity()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::shared::event_display::{EventDisplaySelector, EventSelector};

    #[test]
    fn test_selector_parsing() {
        let sel: EventSelector = "#2".parse().unwrap();
        assert_eq!(sel, EventSelector::Position(2));

        let sel: EventSelector = "foo".parse().unwrap();
        assert_eq!(sel, EventSelector::Exact(b"foo".to_vec()));

        let sel: EventSelector = "*foo".parse().unwrap();
        assert_eq!(
            sel,
            EventSelector::Glob(vec![b"".to_vec(), b"foo".to_vec()])
        );
    }
}
