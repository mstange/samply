//! ETW Event Schema locator and handler
//!
//! This module contains the means needed to locate and interact with the Schema of an ETW event
use std::collections::hash_map::Entry;
use std::rc::Rc;

use once_cell::unsync::OnceCell;
use windows::core::GUID;
use windows::Win32::System::Diagnostics::Etw::{self, EVENT_HEADER_FLAG_64_BIT_HEADER};

use super::etw_types::{DecodingSource, EventRecord, TraceEventInfoRaw};
use super::property::PropertyIter;
use super::tdh_types::Property;
use super::{tdh, FastHashMap};

/// Schema module errors
#[derive(Debug)]
pub enum SchemaError {
    /// Represents a Parser error
    ParseError,
    /// Represents an internal [TdhNativeError]
    ///
    /// [TdhNativeError]: tdh::TdhNativeError
    TdhNativeError(tdh::TdhNativeError),
}

impl From<tdh::TdhNativeError> for SchemaError {
    fn from(err: tdh::TdhNativeError) -> Self {
        SchemaError::TdhNativeError(err)
    }
}

type SchemaResult<T> = Result<T, SchemaError>;

// TraceEvent::RegisteredTraceEventParser::ExternalTraceEventParserState::TraceEventComparer
// doesn't compare the version or level and does different things depending on the kind of event
// https://github.com/microsoft/perfview/blob/5c9f6059f54db41b4ac5c4fc8f57261779634489/src/TraceEvent/RegisteredTraceEventParser.cs#L1338
#[derive(Debug, Eq, PartialEq, Hash)]
struct SchemaKey {
    provider: GUID,
    id: u16,
    version: u8,
    level: u8,
    opcode: u8,
}

// A map from tracelogging schema metdata to ids
struct TraceLoggingProviderIds {
    ids: FastHashMap<Vec<u8>, u16>,
    next_id: u16,
}

impl TraceLoggingProviderIds {
    fn new() -> Self {
        // start the ids at 1 because of 0 is typically the value stored
        TraceLoggingProviderIds {
            ids: FastHashMap::default(),
            next_id: 1,
        }
    }
}

impl SchemaKey {
    pub fn new(event: &EventRecord, locator: &mut SchemaLocator) -> Self {
        // TraceLogging events all use the same id and are distinguished from each other using the metadata.
        // Instead of storing the metadata in the SchemaKey we follow the approach of PerfView and have a side table of synthetic ids keyed on metadata.
        // It might be better to store the metadata in the SchemaKey but then we may want to be careful not to allocate a fresh metadata for every event.
        let mut id = event.EventHeader.EventDescriptor.Id;
        if event.ExtendedDataCount > 0 {
            let extended = unsafe {
                std::slice::from_raw_parts(event.ExtendedData, event.ExtendedDataCount as usize)
            };
            for e in extended {
                if e.ExtType as u32 == Etw::EVENT_HEADER_EXT_TYPE_EVENT_SCHEMA_TL {
                    let provider = locator
                        .tracelogging_providers
                        .entry(event.EventHeader.ProviderId)
                        .or_insert(TraceLoggingProviderIds::new());
                    let data = unsafe {
                        std::slice::from_raw_parts(e.DataPtr as *const u8, e.DataSize as usize)
                    };
                    if let Some(metadata_id) = provider.ids.get(data) {
                        // we want to ensure that our synthetic ids don't overlap with any ids used in the events
                        assert_ne!(id, *metadata_id);
                        id = *metadata_id;
                    } else {
                        provider.ids.insert(data.to_vec(), provider.next_id);
                        id = provider.next_id;
                        provider.next_id += 1;
                    }
                }
            }
        }
        SchemaKey {
            provider: event.EventHeader.ProviderId,
            id,
            version: event.EventHeader.EventDescriptor.Version,
            level: event.EventHeader.EventDescriptor.Level,
            opcode: event.EventHeader.EventDescriptor.Opcode,
        }
    }
}

/// Represents a cache of Schemas already located
///
/// This cache is implemented as a [HashMap] where the key is a combination of the following elements
/// of an [Event Record](https://docs.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record)
/// * EventHeader.ProviderId
/// * EventHeader.EventDescriptor.Id
/// * EventHeader.EventDescriptor.Opcode
/// * EventHeader.EventDescriptor.Version
/// * EventHeader.EventDescriptor.Level
///
/// Credits: [KrabsETW::schema_locator](https://github.com/microsoft/krabsetw/blob/master/krabs/krabs/schema_locator.hpp)
#[derive(Default)]
pub struct SchemaLocator {
    schemas: FastHashMap<SchemaKey, Rc<Schema>>,
    tracelogging_providers: FastHashMap<GUID, TraceLoggingProviderIds>,
}

pub trait EventSchema {
    fn decoding_source(&self) -> DecodingSource;

    fn provider_guid(&self) -> GUID;
    fn event_id(&self) -> u16;
    fn opcode(&self) -> u8;
    fn event_version(&self) -> u8;
    fn provider_name(&self) -> String;
    fn task_name(&self) -> String;
    fn opcode_name(&self) -> String;
    fn level(&self) -> u8;

    fn property_count(&self) -> u32;
    fn property(&self, index: u32) -> Property;

    fn event_message(&self) -> Option<String> {
        None
    }
    fn is_event_metadata(&self) -> bool {
        false
    }
}

impl std::fmt::Debug for SchemaLocator {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl SchemaLocator {
    pub fn new() -> Self {
        SchemaLocator {
            schemas: FastHashMap::default(),
            tracelogging_providers: FastHashMap::default(),
        }
    }

    pub fn add_custom_schema(&mut self, schema: Box<dyn EventSchema>) {
        let key = SchemaKey {
            provider: schema.provider_guid(),
            id: schema.event_id(),
            opcode: schema.opcode(),
            version: schema.event_version(),
            level: schema.level(),
        };
        self.schemas.insert(key, Rc::new(Schema::new(schema)));
    }

    /// Use the `event_schema` function to retrieve the Schema of an ETW Event
    ///
    /// # Arguments
    /// * `event` - The [EventRecord] that's passed to the callback
    ///
    /// # Remark
    /// This is the first function that should be called within a Provider callback, if everything
    /// works as expected this function will return a Result with the [Schema] that represents
    /// the ETW event that triggered the callback
    ///
    /// This function can fail, if it does it will return a [SchemaError]
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    /// };
    /// ```
    pub fn event_schema<'a>(&mut self, event: &'a EventRecord) -> SchemaResult<TypedEvent<'a>> {
        let key = SchemaKey::new(event, self);
        let info = match self.schemas.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let info = Box::new(tdh::schema_from_tdh(event)?);
                // dbg!(info.provider_guid(), info.provider_name(), info.decoding_source());
                // TODO: Cloning for now, should be a reference at some point...
                entry.insert(Rc::new(Schema::new(info)))
            }
        }
        .clone();

        // Some events contain schemas so add them when we find them.
        if info.event_schema.is_event_metadata() {
            let event_info = TraceEventInfoRaw::new(event.user_buffer().to_owned());
            // println!(
            //     "Adding custom schema for {}/{}/{}/{}",
            //     event_info.provider_name(),
            //     event_info.event_id(),
            //     event_info.task_name(),
            //     event_info.opcode_name()
            // );
            self.add_custom_schema(Box::new(event_info));
        }

        Ok(TypedEvent::new(event, info))
    }
}

pub struct Schema {
    pub event_schema: Box<dyn EventSchema>,
    properties: OnceCell<PropertyIter>,
    name: OnceCell<String>,
}

impl Schema {
    fn new(event_schema: Box<dyn EventSchema>) -> Self {
        Schema {
            event_schema,
            properties: OnceCell::new(),
            name: OnceCell::new(),
        }
    }
    pub(crate) fn properties(&self) -> &PropertyIter {
        self.properties.get_or_init(|| PropertyIter::new(self))
    }
    pub(crate) fn name(&self) -> &str {
        self.name.get_or_init(|| {
            format!(
                "{}/{}/{}",
                self.event_schema.provider_name(),
                self.event_schema.task_name(),
                self.event_schema.opcode_name()
            )
        })
    }
}

pub struct TypedEvent<'a> {
    record: &'a EventRecord,
    pub(crate) schema: Rc<Schema>,
}

impl<'a> TypedEvent<'a> {
    pub fn new(record: &'a EventRecord, schema: Rc<Schema>) -> Self {
        TypedEvent { record, schema }
    }

    pub(crate) fn user_buffer(&self) -> &[u8] {
        self.record.user_buffer()
    }

    // Horrible getters FTW!! :D
    // TODO: Not a big fan of this, think a better way..
    pub(crate) fn record(&self) -> &EventRecord {
        self.record
    }

    /// Use the `event_id` function to obtain the EventId of the Event Record
    ///
    /// This getter returns the EventId of the ETW Event that triggered the registered callback
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let event_id = schema.event_id();
    /// };
    /// ```
    pub fn event_id(&self) -> u16 {
        self.record.EventHeader.EventDescriptor.Id
    }

    /// Use the `opcode` function to obtain the Opcode of the Event Record
    ///
    /// This getter returns the opcode of the ETW Event that triggered the registered callback
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let event_id = schema.opcode();
    /// };
    /// ```
    pub fn opcode(&self) -> u8 {
        self.record.EventHeader.EventDescriptor.Opcode
    }

    /// Use the `event_flags` function to obtain the Event Flags of the [EventRecord]
    ///
    /// This getter returns the Event Flags of the ETW Event that triggered the registered callback
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let event_flags = schema.event_flags();
    /// };
    /// ```
    pub fn event_flags(&self) -> u16 {
        self.record.EventHeader.Flags
    }

    pub fn is_64bit(&self) -> bool {
        (self.record.EventHeader.Flags & EVENT_HEADER_FLAG_64_BIT_HEADER as u16) != 0
    }

    /// Use the `event_version` function to obtain the Version of the [EventRecord]
    ///
    /// This getter returns the Version of the ETW Event that triggered the registered callback
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let event_version = schema.event_version();
    /// };
    /// ```  
    pub fn event_version(&self) -> u8 {
        self.record.EventHeader.EventDescriptor.Version
    }

    /// Use the `process_id` function to obtain the ProcessId of the [EventRecord]
    ///
    /// This getter returns the ProcessId of the process that triggered the ETW Event
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let pid = schema.process_id();
    /// };
    /// ```  
    pub fn process_id(&self) -> u32 {
        self.record.EventHeader.ProcessId
    }

    /// Use the `thread_id` function to obtain the ThreadId of the [EventRecord]
    ///
    /// This getter returns the ThreadId of the thread that triggered the ETW Event
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let tid = schema.thread_id();
    /// };
    /// ```  
    pub fn thread_id(&self) -> u32 {
        self.record.EventHeader.ThreadId
    }

    /// Use the `timestamp` function to obtain the TimeStamp of the [EventRecord]
    ///
    /// This getter returns the TimeStamp of the ETW Event
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let timestamp = schema.timestamp();
    /// };
    /// ```  
    pub fn timestamp(&self) -> i64 {
        self.record.EventHeader.TimeStamp
    }

    /// Use the `activity_id` function to obtain the ActivityId of the [EventRecord]
    ///
    /// This getter returns the ActivityId from the ETW Event, this value is used to related Two events
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let activity_id = schema.activity_id();
    /// };
    /// ```
    /// [TraceEventInfo]: super::native::etw_types::TraceEventInfo
    pub fn activity_id(&self) -> GUID {
        self.record.EventHeader.ActivityId
    }

    /// Use the `decoding_source` function to obtain the [DecodingSource] from the [TraceEventInfo]
    ///
    /// This getter returns the DecodingSource from the event, this value identifies the source used
    /// parse the event data
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let decoding_source = schema.decoding_source();
    /// };
    /// ```
    /// [TraceEventInfo]: super::native::etw_types::TraceEventInfo
    pub fn decoding_source(&self) -> DecodingSource {
        self.schema.event_schema.decoding_source()
    }

    /// Use the `provider_name` function to obtain the Provider name from the [TraceEventInfo]
    ///
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let provider_name = schema.provider_name();
    /// };
    /// ```
    /// [TraceEventInfo]: super::native::etw_types::TraceEventInfo
    pub fn provider_name(&self) -> String {
        self.schema.event_schema.provider_name()
    }

    /// Use the `task_name` function to obtain the Task name from the [TraceEventInfo]
    ///
    /// See: [TaskType](https://docs.microsoft.com/en-us/windows/win32/wes/eventmanifestschema-tasktype-complextype)
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let task_name = schema.task_name();
    /// };
    /// ```
    /// [TraceEventInfo]: super::native::etw_types::TraceEventInfo
    pub fn task_name(&self) -> String {
        self.schema.event_schema.task_name()
    }

    /// Use the `opcode_name` function to obtain the Opcode name from the [TraceEventInfo]
    ///
    /// See: [OpcodeType](https://docs.microsoft.com/en-us/windows/win32/wes/eventmanifestschema-opcodetype-complextype)
    /// # Example
    /// ```rust
    /// let my_callback = |record: EventRecord, schema_locator: &mut SchemaLocator| {
    ///     let schema = schema_locator.event_schema(record)?;
    ///     let opcode_name = schema.opcode_name();
    /// };
    /// ```
    /// [TraceEventInfo]: super::native::etw_types::TraceEventInfo
    pub fn opcode_name(&self) -> String {
        self.schema.event_schema.opcode_name()
    }

    pub fn property_count(&self) -> u32 {
        self.schema.event_schema.property_count()
    }

    pub fn property(&self, index: u32) -> Property {
        self.schema.event_schema.property(index)
    }

    pub fn name(&self) -> &str {
        self.schema.name()
    }

    pub fn event_message(&self) -> Option<String> {
        self.schema.event_schema.event_message()
    }
}

impl<'a> PartialEq for TypedEvent<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.schema.event_schema.event_id() == other.schema.event_schema.event_id()
            && self.schema.event_schema.provider_guid() == other.schema.event_schema.provider_guid()
            && self.schema.event_schema.event_version() == other.schema.event_schema.event_version()
    }
}

impl<'a> Eq for TypedEvent<'a> {}
