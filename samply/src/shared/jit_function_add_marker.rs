use fxprof_processed_profile::{
    Category, CategoryColor, MarkerFieldFlags, MarkerFieldFormat, Profile, StaticSchemaMarker,
    StaticSchemaMarkerField, StringHandle,
};

#[derive(Debug, Clone)]
pub struct JitFunctionAddMarker(pub StringHandle);

impl StaticSchemaMarker for JitFunctionAddMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "JitFunctionAdd";

    const CATEGORY: Category<'static> = Category("JIT", CategoryColor::Green);
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

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.0
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }

    fn flow_field_value(&self, _field_index: u32) -> u64 {
        unreachable!()
    }
}
