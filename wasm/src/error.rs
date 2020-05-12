use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
pub struct GetSymbolsError {
    error_type: String,
    error_msg: String,
}

impl From<profiler_get_symbols::GetSymbolsError> for GetSymbolsError {
    fn from(err: profiler_get_symbols::GetSymbolsError) -> Self {
        Self {
            error_type: err.enum_as_string().to_string(),
            error_msg: err.to_string(),
        }
    }
}

impl From<GetSymbolsError> for JsValue {
    fn from(err: GetSymbolsError) -> Self {
        JsValue::from_serde(&err).unwrap()
    }
}
