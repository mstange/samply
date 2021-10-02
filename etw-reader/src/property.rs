//! ETW Event Property information
//!
//! The `property` module expose the basic structures that represent the Properties an Event contains
//! based on it's Schema. This Properties can then be used to parse accordingly their values.
use crate::tdh_types::Property;
use crate::schema::{Schema, TypedEvent};

/// Event Property information
#[derive(Clone, Default, Debug)]
pub struct PropertyInfo {
    /// Property attributes
    pub property: Property,
    pub offset: usize,
    /// Buffer with the Property data
    pub buffer: Vec<u8>,
}

impl PropertyInfo {
    pub fn create(property: Property, offset: usize, buffer: Vec<u8>) -> Self {
        PropertyInfo { property, offset, buffer }
    }
}

pub(crate) struct PropertyIter {
    properties: Vec<Property>,
}

impl PropertyIter {
    fn enum_properties(schema: &Schema, prop_count: u32) -> Vec<Property> {
        let mut properties = Vec::new();
        for i in 0..prop_count {
            properties.push(schema.event_schema.property(i));
        }
        properties
    }

    pub fn new(event: &TypedEvent) -> Self {
        let prop_count = event.property_count();
        let properties = PropertyIter::enum_properties(&event.schema, prop_count);

        PropertyIter { properties }
    }

    pub fn property(&self, index: u32) -> Option<&Property> {
        self.properties.get(index as usize)
    }

    pub fn properties_iter(&self) -> &[Property] {
        &self.properties
    }
}
