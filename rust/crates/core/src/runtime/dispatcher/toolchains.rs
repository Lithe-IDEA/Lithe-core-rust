//! `toolchains.*` commands: the toolchain registry, pins, consumer preferences,
//! discovery and launch instantiation.

use super::plugin_response;
use crate::protocol::{CoreResponse, ToolchainsCommand};
use serde_json::Value;

/// Runs one `toolchains.*` command.
pub(super) fn dispatch(
    command: ToolchainsCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        ToolchainsCommand::Registry => plugin_response(id, payload, |payload| {
            crate::toolchain::registry_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::DecodeReply => plugin_response(id, payload, |payload| {
            crate::toolchain::decode_reply_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::FailureReply => {
            plugin_response(id, payload, crate::toolchain::failure_reply_command)
        }
        ToolchainsCommand::ResolvePins => {
            plugin_response(id, payload, crate::toolchain::resolve_pins_command)
        }
        ToolchainsCommand::EditPins => {
            plugin_response(id, payload, crate::toolchain::edit_pins_command)
        }
        ToolchainsCommand::PinStoragePlan => {
            plugin_response(id, payload, crate::toolchain::pin_storage_plan_command)
        }
        ToolchainsCommand::DecodePinDocument => {
            plugin_response(id, payload, crate::toolchain::decode_pin_document_command)
        }
        ToolchainsCommand::EditPinSources => {
            plugin_response(id, payload, crate::toolchain::edit_pin_sources_command)
        }
        ToolchainsCommand::ResolveConsumerPreferences => plugin_response(
            id,
            payload,
            crate::toolchain::resolve_consumer_preferences_command,
        ),
        ToolchainsCommand::EditConsumerPreferences => plugin_response(
            id,
            payload,
            crate::toolchain::edit_consumer_preferences_command,
        ),
        ToolchainsCommand::Resolve => plugin_response(id, payload, |payload| {
            crate::toolchain::resolve_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::CheckVersion => plugin_response(id, payload, |payload| {
            crate::toolchain::check_version_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::DiscoveryPlan => plugin_response(id, payload, |payload| {
            crate::toolchain::discovery_plan_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::Candidates => plugin_response(id, payload, |payload| {
            crate::toolchain::candidates_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
        ToolchainsCommand::InstantiateLaunch => plugin_response(id, payload, |payload| {
            crate::toolchain::instantiate_launch_command(
                payload,
                lithe_plugin_manifest::validated_manifest_json,
            )
        }),
    }
}
