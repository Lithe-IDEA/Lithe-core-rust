//! Placement of the memory view's readMemory window (`debug.planMemoryWindow`,
//! `debug.locateMemoryWindow`).
//!
//! The memory view reads one bounded window at an address the user typed or at
//! the selected frame's instruction pointer, and highlights the byte at that
//! address. An address written as a number, `0x` hexadecimal or decimal as the
//! Debug Adapter Protocol writes the address of a `readMemory` reply, is read
//! from a little before it, on a multiple of the window alignment, so the byte
//! sits in its row as it would in a full dump for every bytes-per-row choice.
//! Any other reference is opaque: it is read from where it points and never
//! parsed. Core owns the window and where the highlighted byte falls in the
//! reply; hosts own the transport, the rows, the radix and the scrolling.
//!
//! Addresses cross the protocol as strings, never JSON numbers, because a
//! 64-bit address can exceed the JavaScript safe integer range.

use crate::protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Bytes one memory window reads.
const MEMORY_WINDOW_BYTES: u32 = 1_024;
/// Alignment of a numeric window's first byte. Every bytes-per-row choice the
/// views offer (8, 16, 32 and 64) divides it, so rows start on their width.
const MEMORY_WINDOW_ALIGN: u64 = 64;
/// Longest address text accepted, the bound Core applies to every memory reference.
const MAX_ADDRESS_CHARS: usize = 256;

/// Where the memory view reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanMemoryWindowRequest {
    /// The address the user typed, or the selected frame's instruction pointer reference.
    pub(crate) address: String,
}

/// One readMemory window, echoed by the host with the adapter's reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemoryWindow {
    /// The address read at as the address field shows it: a number in lowercase
    /// `0x` hexadecimal, any other reference trimmed and otherwise as given.
    pub(crate) address: String,
    /// The DAP `readMemory` arguments to send as they are.
    pub(crate) request: ReadMemoryArguments,
}

/// DAP `readMemory` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadMemoryArguments {
    pub(crate) memory_reference: String,
    /// Bytes from `memoryReference` to the window's first byte; zero or negative.
    pub(crate) offset: i64,
    pub(crate) count: u32,
}

/// The adapter's reply to one window's `readMemory` request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LocateMemoryWindowRequest {
    pub(crate) window: MemoryWindow,
    /// The reply's `address`, where the returned bytes start; absent when the adapter gave none.
    #[serde(default)]
    pub(crate) start: Option<String>,
    /// How many bytes the reply returned.
    pub(crate) length: u32,
}

/// Where the returned bytes sit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryWindowLocation {
    /// The first returned byte's address in lowercase `0x` hexadecimal; hosts label each row
    /// with its offset added to it. Absent when neither the reply nor the window gives a
    /// number, and rows are labelled with their offset in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base: Option<String>,
    /// The index of the byte at the address read; absent when the reply does not hold it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mark: Option<u32>,
}

/// Places one window at an address.
pub(crate) fn plan_memory_window(
    request: PlanMemoryWindowRequest,
) -> Result<MemoryWindow, CoreError> {
    let address = request.address.trim();
    if address.is_empty()
        || address.chars().count() > MAX_ADDRESS_CHARS
        || address.chars().any(char::is_control)
    {
        return Err(invalid(
            "A memory address must be a short single-line adapter reference.",
        ));
    }
    Ok(match numeric_address(address) {
        Some(value) => {
            let address = format!("0x{value:x}");
            MemoryWindow {
                request: ReadMemoryArguments {
                    memory_reference: address.clone(),
                    // The remainder is below the alignment, so it always fits.
                    offset: -((value % MEMORY_WINDOW_ALIGN) as i64),
                    count: MEMORY_WINDOW_BYTES,
                },
                address,
            }
        }
        None => MemoryWindow {
            address: address.to_string(),
            request: ReadMemoryArguments {
                memory_reference: address.to_string(),
                offset: 0,
                count: MEMORY_WINDOW_BYTES,
            },
        },
    })
}

/// Finds where the bytes of one reply start and which of them is at the address read.
pub(crate) fn locate_memory_window(
    request: LocateMemoryWindowRequest,
) -> Result<MemoryWindowLocation, CoreError> {
    let LocateMemoryWindowRequest {
        window,
        start,
        length,
    } = request;
    if plan_memory_window(PlanMemoryWindowRequest {
        address: window.address.clone(),
    })? != window
    {
        return Err(invalid("The memory window does not match its request."));
    }
    let target = numeric_address(&window.address);
    // The adapter says where the bytes start; without a number for it, they start where the
    // window asked.
    let base = start
        .as_deref()
        .and_then(numeric_address)
        .or_else(|| target.map(|target| target - window.request.offset.unsigned_abs()));
    let index = match (target, base) {
        (Some(target), Some(base)) => target.checked_sub(base),
        // An opaque reference is read from where it points, so its byte comes first.
        (None, _) => Some(0),
        (Some(_), None) => None,
    };
    Ok(MemoryWindowLocation {
        base: base.map(|base| format!("0x{base:x}")),
        mark: index
            .filter(|index| *index < u64::from(length))
            .and_then(|index| u32::try_from(index).ok()),
    })
}

/// Reads an address the way the Debug Adapter Protocol writes one: hexadecimal after
/// `0x`, decimal otherwise. Anything else is an opaque reference.
fn numeric_address(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u64::from_str_radix(digits, 16).ok()
        }
        Some(_) => None,
        None if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => text.parse().ok(),
        None => None,
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/debug/memory-window-v1.json"
        ))
        .unwrap()
    }

    // Each scenario plans one window, then locates the bytes of each reply in it.
    #[test]
    fn memory_windows_match_the_shared_fixture() {
        let fixture = fixture();
        for scenario in fixture["scenarios"].as_array().unwrap() {
            let name = scenario["name"].as_str().unwrap();
            let window = plan_memory_window(PlanMemoryWindowRequest {
                address: scenario["address"].as_str().unwrap().to_string(),
            })
            .unwrap();
            assert_eq!(
                serde_json::to_value(&window).unwrap(),
                scenario["window"],
                "{name}: window"
            );
            for (index, reply) in scenario["replies"].as_array().unwrap().iter().enumerate() {
                let location = locate_memory_window(LocateMemoryWindowRequest {
                    window: window.clone(),
                    start: reply["start"].as_str().map(str::to_string),
                    length: reply["length"].as_u64().unwrap() as u32,
                })
                .unwrap();
                assert_eq!(
                    serde_json::to_value(&location).unwrap(),
                    reply["expected"],
                    "{name}: reply {index}"
                );
            }
        }
    }

    #[test]
    fn unusable_addresses_are_rejected() {
        for address in ["", "   ", "0x1\n2", &"7".repeat(MAX_ADDRESS_CHARS + 1)] {
            assert!(
                plan_memory_window(PlanMemoryWindowRequest {
                    address: address.to_string(),
                })
                .is_err(),
                "{address:?}"
            );
        }
    }

    #[test]
    fn a_tampered_window_is_rejected() {
        let mut window = plan_memory_window(PlanMemoryWindowRequest {
            address: "0x2010".to_string(),
        })
        .unwrap();
        window.request.offset = -4;
        assert!(locate_memory_window(LocateMemoryWindowRequest {
            window,
            start: None,
            length: 16,
        })
        .is_err());
    }

    #[test]
    fn numbers_are_read_as_the_protocol_writes_them() {
        assert_eq!(numeric_address("0x10"), Some(16));
        assert_eq!(numeric_address("0XfF"), Some(255));
        assert_eq!(numeric_address("4096"), Some(4096));
        assert_eq!(numeric_address("0xffffffffffffffff"), Some(u64::MAX));
        for opaque in ["0x", "0x1g", "+12", "-1", "frame:3", "18446744073709551616"] {
            assert_eq!(numeric_address(opaque), None, "{opaque}");
        }
    }
}
