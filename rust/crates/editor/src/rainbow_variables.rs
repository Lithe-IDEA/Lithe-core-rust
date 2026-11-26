//! Stable palette selection for variable tokens identified by the host.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Maximum number of variable ranges accepted in one request.
pub const MAX_VARIABLES: usize = 10_000;
/// Maximum number of UTF-8 bytes in a variable name.
pub const MAX_NAME_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variable {
    /// Inclusive UTF-8 byte offset into the host's document snapshot.
    start: u32,
    /// Exclusive UTF-8 byte offset into the host's document snapshot.
    end: u32,
    /// Exact spelling at the byte range, with case preserved.
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    /// Variable tokens in document order with no overlapping byte ranges.
    variables: Vec<Variable>,
}

#[derive(Serialize)]
struct Color {
    start: u32,
    end: u32,
    /// Stable hash reduced to a slot by the active theme's palette length.
    color: u32,
}

/// Assigns one deterministic color value per variable range. Equal spellings
/// receive the same value across documents, sessions and platforms.
pub fn color_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|_| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid rainbow variable tokens")
    })?;
    if request.variables.len() > MAX_VARIABLES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Rainbow variable tokens exceed 10000 entries",
        ));
    }

    let mut previous_end = 0;
    let mut colors = Vec::with_capacity(request.variables.len());
    for variable in request.variables {
        let name_bytes = variable.name.len();
        if variable.start < previous_end
            || variable.start >= variable.end
            || name_bytes > MAX_NAME_BYTES
            || (variable.end - variable.start) as usize != name_bytes
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Rainbow variable ranges must be ordered, non-overlapping and match their bounded UTF-8 names",
            ));
        }
        previous_end = variable.end;
        colors.push(Color {
            start: variable.start,
            end: variable.end,
            color: name_color(&variable.name),
        });
    }
    Ok(json!({ "colors": colors }))
}

fn name_color(name: &str) -> u32 {
    // FNV-1a over the original UTF-8 bytes keeps palettes stable when
    // document ranges move, regardless of process-local hash seeds.
    let mut hash = 0x811c_9dc5_u32;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Murmur3 fmix32 diffuses the low bits before the host takes a palette
    // modulus, so similar spellings do not systematically share a slot.
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_names_distribute_across_eight_color_slots() {
        let names = ["user1", "user9", "a", "i", "arg0", "arg8", "x", "p"];
        let colors = names.map(name_color);
        assert_eq!(
            colors,
            [
                3_385_640_019,
                1_105_349_995,
                444_641_715,
                1_311_273_838,
                2_657_546_199,
                1_664_241_451,
                794_621_484,
                2_710_276_834
            ]
        );
        let slots: std::collections::BTreeSet<_> =
            colors.map(|color| color % 8).into_iter().collect();
        assert!(slots.len() >= 5, "similar spellings use {slots:?}");
    }

    #[test]
    fn enforces_token_and_name_budgets() {
        let mut variables: Vec<Value> = (0..MAX_VARIABLES)
            .map(|start| json!({"start":start,"end":start+1,"name":"a"}))
            .collect();
        let result = color_command(json!({"variables":&variables})).unwrap();
        let colors = result["colors"].as_array().unwrap();
        assert_eq!(colors.len(), MAX_VARIABLES);
        assert_eq!(colors[MAX_VARIABLES - 1]["color"], 444_641_715_u32);

        variables.push(json!({"start":MAX_VARIABLES,"end":MAX_VARIABLES+1,"name":"a"}));
        let error = color_command(json!({"variables":variables})).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));

        for (name_bytes, valid) in [(MAX_NAME_BYTES, true), (MAX_NAME_BYTES + 1, false)] {
            let result = color_command(json!({
                "variables":[{"start":0,"end":name_bytes,"name":"a".repeat(name_bytes)}]
            }));
            assert_eq!(result.is_ok(), valid);
        }
    }
}
