use fxprof_processed_profile::{
    CategoryHandle, MarkerFieldFormat, MarkerFieldSchema, MarkerLocation, MarkerSchema,
    MarkerStaticField, Profile, StaticSchemaMarker, StringHandle,
};

#[derive(Debug, Clone)]
pub struct JitFunctionAddMarker(pub StringHandle);

impl StaticSchemaMarker for JitFunctionAddMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "JitFunctionAdd";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.n}".into()),
            tooltip_label: Some("{marker.data.n}".into()),
            table_label: Some("{marker.data.n}".into()),
            fields: vec![MarkerFieldSchema {
                key: "n".into(),
                label: "Function".into(),
                format: MarkerFieldFormat::String,
                searchable: true,
            }],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted when a JIT function is added to the process.".into(),
            }],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("JitFunctionAdd")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.0
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}
