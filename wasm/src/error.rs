use samply_api::samply_symbols;
use serde::Serialize;
use std::fmt::{self, Display};
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
pub struct GetSymbolsError {
    name: String,
    message: String,
}

impl From<samply_symbols::Error> for GetSymbolsError {
    fn from(err: samply_symbols::Error) -> Self {
        Self {
            name: err.enum_as_string().to_string(),
            message: err.to_string(),
        }
    }
}

impl From<GetSymbolsError> for JsValue {
    fn from(err: GetSymbolsError) -> Self {
        JsValue::from_serde(&err).unwrap()
    }
}

#[derive(Debug)]
pub struct JsValueError {
    name: String,
    message: String,
}

impl Display for JsValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl From<JsValue> for JsValueError {
    fn from(error: JsValue) -> Self {
        let error = js_sys::Error::from(error);
        let name: String = error.name().into();
        let message: String = error.message().into();
        Self { name, message }
    }
}

impl std::error::Error for JsValueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

#[derive(Debug)]
pub struct GenericError(pub &'static str);

impl std::error::Error for GenericError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl Display for GenericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
