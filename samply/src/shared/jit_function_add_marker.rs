use fxprof_processed_profile::{
    Category, CategoryColor, Marker, MarkerField, Profile, Schema, StringHandle,
};

#[derive(Debug, Clone)]
pub struct JitFunctionAddMarker(pub StringHandle);

impl Marker for JitFunctionAddMarker {
    type FieldsType = StringHandle;

    const UNIQUE_MARKER_TYPE_NAME: &'static str = "JitFunctionAdd";

    const CATEGORY: Category<'static> = Category("JIT", CategoryColor::Green);
    const DESCRIPTION: Option<&'static str> =
        Some("Emitted when a JIT function is added to the process.");

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.n}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.n}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.data.n}");

    const FIELDS: Schema<Self::FieldsType> = Schema(MarkerField::string("n", "Function"));

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("JitFunctionAdd")
    }

    fn field_values(&self) -> StringHandle {
        self.0
    }
}
