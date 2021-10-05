//! ETW Event Property information
//!
//! The `property` module expose the basic structures that represent the Properties an Event contains
//! based on it's Schema. This Properties can then be used to parse accordingly their values.
use crate::FastHashMap;
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
    pub (crate) name_to_indx: FastHashMap<String, usize>
}

impl PropertyIter {
    pub fn new(schema: &Schema) -> Self {
        let prop_count = schema.event_schema.property_count();
        let mut properties = Vec::new();
        let mut name_to_indx = FastHashMap::default();
        for i in 0..prop_count {
            let prop = schema.event_schema.property(i);
            name_to_indx.insert(prop.name.clone(), i as usize);
            properties.push(prop);
        }

        PropertyIter { properties, name_to_indx }
    }

    pub fn property(&self, index: u32) -> Option<&Property> {
        self.properties.get(index as usize)
    }

    pub fn properties_iter(&self) -> &[Property] {
        &self.properties
    }
}
