use fxprof_processed_profile::{
    CategoryHandle, MarkerFieldFlags, MarkerFieldFormat, Profile, StaticSchemaMarker,
    StaticSchemaMarkerField, StringHandle,
};

#[derive(Debug, Clone)]
pub struct JitFunctionAddMarker(pub StringHandle);

impl StaticSchemaMarker for JitFunctionAddMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "JitFunctionAdd";

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted when a JIT function is added to the process.");

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.n}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.n}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.data.n}");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "n",
        label: "Function",
        format: MarkerFieldFormat::String,
        flags: MarkerFieldFlags::SEARCHABLE,
    }];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("JitFunctionAdd")
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
