//! Selection and byte-exact reconstruction of marketplace ZIP deltas.
//!
//! The main index is authoritative for compatibility and the target hash. A
//! companion can only describe how to obtain those same bytes. The host owns
//! networking, cache admission and the private staging directory.

use super::{is_platform, is_sha256, outcome, parse_repository, parse_version, release_asset_tag};
use lithe_plugin_model::{decode_payload, is_plugin_id, PluginRejection, Validation};
use lithe_protocol::{cancellation, CoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};

const FORMAT: &str = "zstd-prefix";
const SUFFIX: &str = ".zip.zst";
const MAX_ZIP_BYTES: u64 = 64 << 20;
const MAX_WINDOW_LOG: u32 = 27;
const MAX_ENTRIES: usize = 20_000;
const CHUNK_BYTES: usize = 32 << 10;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Patch {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    platform: String,
    from_version: String,
    #[serde(rename = "fromSHA256")]
    from_sha256: String,
    to_version: String,
    #[serde(rename = "toSHA256")]
    to_sha256: String,
    to_size: u64,
    format: String,
    #[serde(rename = "downloadURL")]
    download_url: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Index {
    schema_version: u64,
    patches: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Select {
    index: Index,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    platform: String,
    repository: String,
    from_version: String,
    #[serde(rename = "fromSHA256")]
    from_sha256: String,
    to_version: String,
    #[serde(rename = "toSHA256")]
    to_sha256: String,
    #[serde(rename = "archiveURL")]
    archive_url: String,
}

fn invalid(message: impl Into<String>) -> PluginRejection {
    PluginRejection::new("plugin.delta.invalid", message)
}

fn valid_sizes(patch: &Patch) -> bool {
    patch.to_size > 0
        && patch.to_size <= MAX_ZIP_BYTES
        && patch.size > 0
        && patch.size <= patch.to_size * 3 / 4
}

/// Returns a single supported patch for the exact approved base and selected target,
/// or `patch: null` when the companion has no applicable entry.
pub fn select_delta_command(payload: Value) -> Result<Value, CoreError> {
    let request: Select = decode_payload(payload, "plugins.marketplace.selectDelta")?;
    Ok(outcome("patch", select(request)))
}

fn select(request: Select) -> Validation<Value> {
    if request.index.schema_version != 0 || request.index.patches.len() > MAX_ENTRIES {
        return Err(invalid("Unsupported or oversized plugin delta index."));
    }
    let Some(repository) = parse_repository(&request.repository) else {
        return Err(invalid("The plugin repository is invalid."));
    };
    let tag = release_asset_tag(&request.archive_url, &repository, ".zip")
        .ok_or_else(|| invalid("The target archive does not belong to the plugin release."))?;
    let from = parse_version(&request.from_version);
    let to = parse_version(&request.to_version);
    if !matches!((from, to), (Some(from), Some(to)) if from < to)
        || !is_plugin_id(&request.plugin_id)
        || !is_platform(&request.platform)
        || !is_sha256(&request.from_sha256)
        || !is_sha256(&request.to_sha256)
    {
        return Err(invalid(
            "The plugin delta request has invalid identity facts.",
        ));
    }
    let mut selected = None;
    for entry in request.index.patches {
        if entry["pluginID"] != request.plugin_id
            || entry["platform"] != request.platform
            || entry["fromVersion"] != request.from_version
            || entry["fromSHA256"] != request.from_sha256
            || entry["toVersion"] != request.to_version
            || entry["toSHA256"] != request.to_sha256
            || entry["format"] != FORMAT
        {
            continue;
        }
        let patch: Patch = serde_json::from_value(entry)
            .map_err(|_| invalid("The matching plugin delta entry is malformed."))?;
        if !valid_sizes(&patch)
            || !is_sha256(&patch.sha256)
            || release_asset_tag(&patch.download_url, &repository, SUFFIX).as_deref()
                != Some(tag.as_str())
        {
            return Err(invalid("The matching plugin delta asset is invalid."));
        }
        if selected.replace(patch).is_some() {
            return Err(invalid("The plugin delta selection is ambiguous."));
        }
    }
    Ok(json!(selected))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Apply {
    patch: Patch,
    base_path: PathBuf,
    patch_path: PathBuf,
    output_path: PathBuf,
}

/// Rebuilds a selected marketplace archive from the base and patch files, in a
/// fresh path in the host's private staging directory. If the command fails,
/// the caller owns deletion of the staging directory including any partial output.
pub fn apply_delta_command(payload: Value) -> Result<Value, CoreError> {
    let request: Apply = decode_payload(payload, "plugins.marketplace.applyDelta")?;
    Ok(outcome("applied", apply(&request)?))
}

fn apply(request: &Apply) -> Result<Validation<Value>, CoreError> {
    cancellation::check()?;
    let patch = &request.patch;
    if patch.format != FORMAT
        || !valid_sizes(patch)
        || ![&patch.from_sha256, &patch.to_sha256, &patch.sha256]
            .iter()
            .all(|digest| is_sha256(digest))
        || ![
            &request.base_path,
            &request.patch_path,
            &request.output_path,
        ]
        .iter()
        .all(|path| path.is_absolute())
    {
        return Ok(Err(invalid(
            "The plugin delta application request is invalid.",
        )));
    }

    // Keep the verified base snapshot in one bounded buffer. The patch is verified
    // and decoded from the same file handle. The host owns its private staging.
    let result = rebuild(request);
    if let Err(error) = &result {
        if let Some(interruption) = error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<Interrupted>())
        {
            return Err(interruption.0.clone());
        }
    }
    cancellation::check()?;
    Ok(result
        .map(|()| json!(true))
        .map_err(|error| invalid(error.to_string())))
}

fn regular_file(path: &Path) -> std::io::Result<File> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other(
            "Expected a regular plugin delta file.",
        ));
    }
    File::open(path)
}

#[derive(Debug)]
struct Interrupted(CoreError);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0.message)
    }
}

impl std::error::Error for Interrupted {}

fn check_progress() -> std::io::Result<()> {
    cancellation::check().map_err(|error| std::io::Error::other(Interrupted(error)))
}

fn copy_verified<R: Read, W: Write>(
    mut source: R,
    mut target: W,
    limit: u64,
    exact_size: Option<u64>,
    expected: &str,
) -> std::io::Result<()> {
    let mut hash = Sha256::new();
    let mut length = 0;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_progress()?;
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > limit {
            return Err(std::io::Error::other(
                "Plugin delta file exceeds its size limit.",
            ));
        }
        hash.update(&buffer[..count]);
        target.write_all(&buffer[..count])?;
    }
    if exact_size.is_some_and(|size| size != length) || format!("{:x}", hash.finalize()) != expected
    {
        return Err(std::io::Error::other(
            "Plugin delta length or SHA-256 does not match.",
        ));
    }
    Ok(())
}

fn rebuild(request: &Apply) -> std::io::Result<()> {
    let patch = &request.patch;
    let base = regular_file(&request.base_path)?;
    let size = base.metadata()?.len();
    if size == 0 || size > MAX_ZIP_BYTES {
        return Err(std::io::Error::other(
            "Plugin delta base exceeds its size limit.",
        ));
    }
    let mut prefix = Vec::with_capacity(size as usize);
    copy_verified(
        base,
        &mut prefix,
        MAX_ZIP_BYTES,
        Some(size),
        &patch.from_sha256,
    )?;

    let mut patch_file = regular_file(&request.patch_path)?;
    copy_verified(
        &mut patch_file,
        std::io::sink(),
        patch.size,
        Some(patch.size),
        &patch.sha256,
    )?;
    patch_file.rewind()?;
    let mut delta =
        zstd::stream::read::Decoder::with_ref_prefix(BufReader::new(patch_file), &prefix)?;
    delta.window_log_max(MAX_WINDOW_LOG)?;
    let mut output = File::create_new(&request.output_path)?;
    copy_verified(
        delta,
        &mut output,
        patch.to_size,
        Some(patch.to_size),
        &patch.to_sha256,
    )?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        let index: Value = serde_json::from_str(include_str!(
            "../../../../shared/plugins/contract/v1/fixtures/marketplace/delta-index.json"
        ))
        .unwrap();
        json!({
            "pluginID": "io.example.delta", "platform": "darwin-arm64",
            "repository": "https://github.com/example/delta", "fromVersion": "1.0.0",
            "fromSHA256": "a".repeat(64), "toVersion": "1.1.0", "toSHA256": "b".repeat(64),
            "archiveURL": "https://github.com/example/delta/releases/download/v1.1.0/delta-darwin-arm64.zip",
            "index": index,
        })
    }

    #[test]
    fn delta_needs_exact_platform_and_digests() {
        let body = request();
        let selected = select_delta_command(body.clone()).unwrap();
        assert_eq!(selected["patch"], body["index"]["patches"][0]);
        for (key, value) in [
            ("platform", json!("windows-x64")),
            ("fromSHA256", json!("f".repeat(64))),
            ("toSHA256", json!("f".repeat(64))),
        ] {
            let mut different = body.clone();
            different[key] = value;
            assert_eq!(
                select_delta_command(different).unwrap(),
                json!({ "status": "ok", "patch": null })
            );
        }
    }

    #[test]
    fn delta_refuses_wrong_release_oversize_and_duplicate_entries() {
        let body = request();
        for (field, value) in [
            (
                "downloadURL",
                json!("https://github.com/other/delta/releases/download/v1.1.0/file.zip.zst"),
            ),
            (
                "downloadURL",
                json!("https://github.com/example/delta/releases/download/v1.0.0/file.zip.zst"),
            ),
            ("size", json!(4096)),
        ] {
            let mut invalid = body.clone();
            invalid["index"]["patches"][0][field] = value;
            let reply = select_delta_command(invalid).unwrap();
            assert_eq!(reply["error"]["code"], "plugin.delta.invalid");
        }
        let mut duplicate = body;
        let patches = duplicate["index"]["patches"].as_array_mut().unwrap();
        patches.push(patches[0].clone());
        let reply = select_delta_command(duplicate).unwrap();
        assert_eq!(reply["error"]["code"], "plugin.delta.invalid");
    }
}
