//! ETW Event Schema locator and handler
//!
//! This module contains the means needed to locate and interact with the Schema of an ETW event
use crate::etw_types::{DecodingSource, EventRecord, TraceEventInfoRaw};
use crate::tdh;
use crate::tdh_types::Property;
use std::collections::HashMap;
use std::sync::Arc;
use windows::Guid;

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

#[derive(Debug, Eq, PartialEq, Hash)]
struct SchemaKey {
    // For now, lazy to wrap Guid around an implement Hash
    // TODO: wrap Guid and implement hash
    provider: String,
    id: u16,
    opcode: u8,
    version: u8,
    level: u8,
}

impl SchemaKey {
    pub fn new(event: &EventRecord) -> Self {
        let provider = format!("{:?}", event.EventHeader.ProviderId);
        SchemaKey {
            provider,
            id: event.EventHeader.EventDescriptor.Id,
            opcode: event.EventHeader.EventDescriptor.Opcode,
            version: event.EventHeader.EventDescriptor.Version,
            level: event.EventHeader.EventDescriptor.Level,
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
    schemas: HashMap<SchemaKey, Arc<dyn EventSchema>>,
}

pub trait EventSchema {
    fn decoding_source(&self) -> DecodingSource;
    
    fn provider_guid(&self) -> Guid;
    fn event_id(&self) -> u16;
    fn opcode(&self) -> u8;
    fn event_version(&self) -> u8;
    fn provider_name(&self) -> String;
    fn task_name(&self) -> String;
    fn opcode_name(&self) -> String;
    
    fn property_count(&self) -> u32;
    fn property(&self, index: u32) -> Property;
}



impl std::fmt::Debug for SchemaLocator {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl SchemaLocator {
    pub fn new() -> Self {
        SchemaLocator {
            schemas: HashMap::new(),
        }
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
    pub fn event_schema(&mut self, event: EventRecord) -> SchemaResult<Schema> {
        let key = SchemaKey::new(&event);
        let info: Arc<dyn EventSchema>;

        if !self.schemas.contains_key(&key) {
            info = Arc::new(tdh::schema_from_tdh(event.clone())?);
            // TODO: Cloning for now, should be a reference at some point...
            self.schemas.insert(key, info.clone());
        } else {
            info = self.schemas.get(&key).unwrap().clone();
        }

        Ok(Schema::new(event, info))
    }
}

/// Represents a Schema
///
/// This structure holds a [TraceEventInfo](https://docs.microsoft.com/en-us/windows/win32/api/tdh/ns-tdh-trace_event_info)
/// which let us obtain information from the ETW event
pub struct Schema {
    record: EventRecord,
    schema: Arc<dyn EventSchema>,
}

impl Schema {
    pub fn new(record: EventRecord, schema: Arc<dyn EventSchema>) -> Self {
        Schema { record, schema }
    }

    pub(crate) fn user_buffer(&self) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts(
                self.record.UserData as *mut _,
                self.record.UserDataLength.into(),
            )
            .to_vec()
        }
    }

    // Horrible getters FTW!! :D
    // TODO: Not a big fan of this, think a better way..
    pub(crate) fn record(&self) -> EventRecord {
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
    /// [TraceEventInfo]: crate::native::etw_types::TraceEventInfo
    pub fn activity_id(&self) -> Guid {
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
    /// [TraceEventInfo]: crate::native::etw_types::TraceEventInfo
    pub fn decoding_source(&self) -> DecodingSource {
        self.schema.decoding_source()
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
    /// [TraceEventInfo]: crate::native::etw_types::TraceEventInfo
    pub fn provider_name(&self) -> String {
        self.schema.provider_name()
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
    /// [TraceEventInfo]: crate::native::etw_types::TraceEventInfo
    pub fn task_name(&self) -> String {
        self.schema.task_name()
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
    /// [TraceEventInfo]: crate::native::etw_types::TraceEventInfo
    pub fn opcode_name(&self) -> String {
        self.schema.opcode_name()
    }

    pub fn property_count(&self) -> u32 {
        self.schema.property_count()
    }

    pub fn property(&self, index: u32) -> Property {
        self.schema.property(index)
    }
}

impl PartialEq for Schema {
    fn eq(&self, other: &Self) -> bool {
        self.schema.event_id() == other.schema.event_id()
            && self.schema.provider_guid() == other.schema.provider_guid()
            && self.schema.event_version() == other.schema.event_version()
    }
}

impl Eq for Schema {}

#[cfg(test)]
mod test {
    use super::*;

    fn test_getters() {
        todo!()
    }

    fn test_schema_key() {
        todo!()
    }
}
