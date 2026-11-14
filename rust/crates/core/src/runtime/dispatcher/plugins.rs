//! `plugins.*` commands: plugin manifests, native UI and web content, plugin settings,
//! installed plugins and the marketplace.

use super::plugin_response;
use crate::protocol::{CoreResponse, PluginsCommand};
use serde_json::Value;

/// Runs one `plugins.*` command.
pub(super) fn dispatch(
    command: PluginsCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        PluginsCommand::NativeUIValidateData => {
            plugin_response(id, payload, lithe_plugin_native_ui::validate_data_command)
        }
        PluginsCommand::WebOpen => {
            plugin_response(id, payload, lithe_plugin_web_content::open_command)
        }
        PluginsCommand::WebClose => {
            plugin_response(id, payload, lithe_plugin_web_content::close_command)
        }
        PluginsCommand::WebResource => {
            plugin_response(id, payload, lithe_plugin_web_content::resource_command)
        }
        PluginsCommand::WebDocument => {
            plugin_response(id, payload, lithe_plugin_web_content::document_command)
        }
        PluginsCommand::WebRequest => {
            plugin_response(id, payload, lithe_plugin_web_content::request_command)
        }
        PluginsCommand::WebReply => {
            plugin_response(id, payload, lithe_plugin_web_content::reply_command)
        }
        PluginsCommand::WebEvent => {
            plugin_response(id, payload, lithe_plugin_web_content::event_command)
        }
        PluginsCommand::DeclarativeValidate => {
            plugin_response(id, payload, lithe_plugin_manifest::validate_command)
        }
        PluginsCommand::DeclarativeRunPlan => {
            plugin_response(id, payload, lithe_plugin_manifest::run_plan_command)
        }
        PluginsCommand::DeclarativeDiscoverTests => {
            plugin_response(id, payload, lithe_plugin_manifest::discover_tests_command)
        }
        PluginsCommand::DeclarativeTestPlan => {
            plugin_response(id, payload, lithe_plugin_manifest::test_plan_command)
        }
        PluginsCommand::DeclarativeLanguageServerLaunchPlan => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::language_server_launch_plan_command,
        ),
        PluginsCommand::LanguageServersDecodeLaunch => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::decode_language_server_launch_command,
        ),
        PluginsCommand::LanguageServersStateRetention => plugin_response(
            id,
            payload,
            lithe_plugin_language_servers::state_retention_command,
        ),
        PluginsCommand::ContentProvidersDecode => plugin_response(
            id,
            payload,
            lithe_plugin_content_providers::decode_content_command,
        ),
        PluginsCommand::DeclarativeResolveSettings => {
            plugin_response(id, payload, lithe_plugin_manifest::resolve_settings_command)
        }
        PluginsCommand::InstalledSelect => {
            plugin_response(id, payload, lithe_plugin_model::installed::select_command)
        }
        PluginsCommand::SettingsCatalog => {
            plugin_response(id, payload, lithe_plugin_manifest::settings_catalog_command)
        }
        PluginsCommand::SettingsNavigation => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::settings_navigation_command,
        ),
        PluginsCommand::SettingsDelivery => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::settings_delivery_command,
        ),
        PluginsCommand::DeclarativeSettingsState => {
            plugin_response(id, payload, lithe_plugin_manifest::settings_state_command)
        }
        PluginsCommand::DeclarativeSettingsActionPlan => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::settings_action_plan_command,
        ),
        PluginsCommand::DeclarativeEditSettings => {
            plugin_response(id, payload, lithe_plugin_manifest::edit_settings_command)
        }
        PluginsCommand::DeclarativeWorkspaceConfiguration => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::workspace_configuration_command,
        ),
        PluginsCommand::DeclarativeDecodeGrammar => {
            plugin_response(id, payload, lithe_plugin_manifest::decode_grammar_command)
        }
        PluginsCommand::EditorActionsDecode => {
            plugin_response(id, payload, lithe_plugin_editor::actions::decode_command)
        }
        PluginsCommand::HostApiDescribe => {
            plugin_response(id, payload, lithe_plugin_rpc::host_api::describe_command)
        }
        PluginsCommand::HostApiAuthorize => {
            plugin_response(id, payload, lithe_plugin_rpc::host_api::authorize_command)
        }
        PluginsCommand::EditorProvidersDecode => {
            plugin_response(id, payload, lithe_plugin_editor::providers::decode_command)
        }
        PluginsCommand::MarketplaceValidateIndex => plugin_response(
            id,
            payload,
            lithe_plugin_marketplace::validate_index_command,
        ),
        PluginsCommand::MarketplaceResolveSource => plugin_response(
            id,
            payload,
            lithe_plugin_marketplace::resolve_source_command,
        ),
        PluginsCommand::MarketplaceInstallReview => plugin_response(
            id,
            payload,
            lithe_plugin_marketplace::install_review_command,
        ),
    }
}
