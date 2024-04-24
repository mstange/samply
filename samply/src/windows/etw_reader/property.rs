//! ETW Event Property information
//!
//! The `property` module expose the basic structures that represent the Properties an Event contains
//! based on it's Schema. This Properties can then be used to parse accordingly their values.
use super::FastHashMap;
use super::tdh_types::Property;
use super::schema::Schema;

/// Event Property information
#[derive(Clone, Debug)]
pub struct PropertyInfo<'a> {
    /// Property attributes
    pub property: &'a Property,
    pub offset: usize,
    /// Buffer with the Property data
    pub buffer: &'a [u8],
}

impl<'a> PropertyInfo<'a> {
    pub fn create(property: &'a Property, offset: usize, buffer: &'a [u8]) -> Self {
        PropertyInfo { property, offset, buffer }
    }
}

pub(crate) struct PropertyIter {
    properties: Vec<Property>,
    pub (crate) name_to_indx: FastHashMap<String, usize>,
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

    pub fn property(&self, index: usize) -> Option<&Property> {
        self.properties.get(index)
    }

    pub fn properties_iter(&self) -> &[Property] {
        &self.properties
    }
}
