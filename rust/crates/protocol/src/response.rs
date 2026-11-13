//! Success-or-failure envelope returned across the JSON and C boundaries.

use crate::CoreError;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// Stable success-or-failure envelope returned across the JSON and C boundaries.
pub struct CoreResponse {
    /// Correlation identifier copied from the request, when supplied.
    pub id: Option<String>,
    /// Discriminator that determines whether `data` or `error` is present.
    pub ok: bool,
    /// Successful command payload. It is omitted for failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseData>,
    /// Structured failure. It is omitted for successful responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CoreError>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
/// Payload wrapper that preserves each command's existing JSON shape.
pub enum ResponseData {
    /// A command-specific JSON value serialized without an additional tag.
    Json(Value),
}

impl CoreResponse {
    /// Reports whether the response carries successful data.
    pub fn is_success(&self) -> bool {
        self.ok
    }

    /// Builds a successful response while preserving the caller's identifier.
    pub fn success(id: Option<String>, data: impl Into<Value>) -> Self {
        Self {
            id,
            ok: true,
            data: Some(ResponseData::Json(data.into())),
            error: None,
        }
    }

    /// Builds a failed response with no partially successful data attached.
    pub fn failure(id: Option<String>, error: CoreError) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(error),
        }
    }
}
