use fxprof_processed_profile::{
    MarkerDynamicField, MarkerFieldFormat, MarkerLocation, MarkerSchema, MarkerSchemaField,
    MarkerStaticField, ProfilerMarker,
};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct JitFunctionAddMarker(pub String);

impl ProfilerMarker for JitFunctionAddMarker {
    const MARKER_TYPE_NAME: &'static str = "JitFunctionAdd";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "functionName": self.0
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.functionName}"),
            tooltip_label: Some("{marker.data.functionName}"),
            table_label: Some("{marker.data.functionName}"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "functionName",
                    label: "Function",
                    format: MarkerFieldFormat::String,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Emitted when a JIT function is added to the process.",
                }),
            ],
        }
    }
}