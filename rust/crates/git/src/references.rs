//! Validation of user-supplied revisions, references and branch, tag and stash names.

use super::*;

#[derive(Clone)]
pub(crate) struct ValidatedGitReference {
    pub(crate) full_name: String,
    pub(crate) short_name: String,
    pub(crate) kind: String,
}

pub(crate) fn invalid_git_reference() -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git reference")
}

pub(crate) fn validated_git_reference(
    root: &str,
    reference: &GitReferenceRequest,
) -> Result<ValidatedGitReference, CoreError> {
    if reference.full_name.contains(['\0', '\n', '\r'])
        || reference.short_name.contains(['\0', '\n', '\r'])
        || reference.full_name.chars().any(char::is_whitespace)
        || reference.short_name.trim() != reference.short_name
    {
        return Err(invalid_git_reference());
    }

    let prefix = match reference.kind.as_str() {
        "local" => "refs/heads/",
        "remote" => "refs/remotes/",
        "tag" => "refs/tags/",
        _ => return Err(invalid_git_reference()),
    };
    let expected_short_name = reference
        .full_name
        .strip_prefix(prefix)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_git_reference)?;
    if expected_short_name != reference.short_name {
        return Err(invalid_git_reference());
    }

    let checked = execute_git_readonly(
        root,
        &["check-ref-format".into(), reference.full_name.clone()],
        None,
    )?;
    if checked.exit_code != 0 {
        return Err(invalid_git_reference());
    }
    if reference.kind == "remote" {
        let (_, branch) = mutations::remote_branch_components(root, &reference.full_name)?;
        if branch == "HEAD" {
            return Err(invalid_git_reference());
        }
    }

    Ok(ValidatedGitReference {
        full_name: reference.full_name.clone(),
        short_name: reference.short_name.clone(),
        kind: reference.kind.clone(),
    })
}

fn legacy_checkout_reference(
    root: &str,
    reference: &str,
    kind: &str,
) -> Result<ValidatedGitReference, CoreError> {
    let (full_name, short_name) = match kind {
        "local" => {
            let short_name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
            (format!("refs/heads/{short_name}"), short_name.to_string())
        }
        "remote" => {
            let short_name = reference
                .strip_prefix("refs/remotes/")
                .ok_or_else(invalid_git_reference)?;
            (reference.to_string(), short_name.to_string())
        }
        "tag" => {
            let short_name = reference.strip_prefix("refs/tags/").unwrap_or(reference);
            (format!("refs/tags/{short_name}"), short_name.to_string())
        }
        _ => return Err(invalid_git_reference()),
    };
    validated_git_reference(
        root,
        &GitReferenceRequest {
            full_name,
            short_name,
            kind: kind.to_string(),
        },
    )
}

pub(crate) fn optional_write_request_reference(
    root: &str,
    request: &GitWriteRequest,
) -> Result<Option<String>, CoreError> {
    if let Some(reference) = request.git_reference.as_ref() {
        return validated_git_reference(root, reference).map(|value| Some(value.full_name));
    }
    request
        .reference
        .as_deref()
        .map(|reference| validated_reference(Some(reference)))
        .transpose()
}

pub(crate) fn write_request_reference(
    root: &str,
    request: &GitWriteRequest,
) -> Result<String, CoreError> {
    // Branch restore records carry the deleted commit object ID rather than a
    // live ref. Accept that exact revision only for createBranch; every other
    // mutation continues to require a typed, existing Git reference.
    if request.operation == "createBranch" {
        if let Some(reference) = request.git_reference.as_ref() {
            if reference.full_name == reference.short_name {
                if let Ok(revision) = validated_revision(Some(&reference.full_name)) {
                    return Ok(revision);
                }
            }
        }
    }
    optional_write_request_reference(root, request)?
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Missing Git reference"))
}

pub(crate) fn checkout_request_reference(
    root: &str,
    request: &GitWriteRequest,
) -> Result<ValidatedGitReference, CoreError> {
    if let Some(reference) = request.git_reference.as_ref() {
        return validated_git_reference(root, reference);
    }
    let reference = validated_reference(request.reference.as_deref())?;
    let kind = required_text(request.reference_kind.as_deref(), "reference kind")?;
    legacy_checkout_reference(root, &reference, &kind)
}

pub(crate) fn request_reference(
    root: &str,
    typed: Option<&GitReferenceRequest>,
    legacy: Option<&str>,
) -> Result<String, CoreError> {
    if let Some(reference) = typed {
        return validated_git_reference(root, reference).map(|value| value.full_name);
    }
    validated_reference(legacy)
}

pub(crate) fn typed_reference_range(
    root: &str,
    base: Option<&GitReferenceRequest>,
    target: Option<&GitReferenceRequest>,
    legacy: Option<&str>,
) -> Result<Option<String>, CoreError> {
    match (base, target) {
        (Some(base), Some(target)) => {
            if legacy.is_some() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Git comparison cannot combine typed and legacy references",
                ));
            }
            let base = validated_git_reference(root, base)?;
            let target = validated_git_reference(root, target)?;
            Ok(Some(format!("{}..{}", base.full_name, target.full_name)))
        }
        (None, Some(_)) => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git comparison target requires a base reference",
        )),
        (Some(base), None) => {
            if legacy.is_some() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Git comparison cannot combine typed and legacy references",
                ));
            }
            Ok(Some(validated_git_reference(root, base)?.full_name))
        }
        (None, None) => legacy
            .map(|value| validated_reference(Some(value)))
            .transpose(),
    }
}

pub(crate) fn validated_revision(value: Option<&str>) -> Result<String, CoreError> {
    let value = required_text(value, "revision")?;
    validate_revision(&value)?;
    Ok(value)
}

pub(crate) fn validated_reference(value: Option<&str>) -> Result<String, CoreError> {
    let value = required_text(value, "reference")?;
    if value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git reference",
        ));
    }
    Ok(value)
}

pub(crate) fn validated_stash_reference(value: Option<&str>) -> Result<String, CoreError> {
    let value = required_text(value, "stash reference")?;
    if value.starts_with('-') || value.contains(char::is_whitespace) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git stash reference",
        ));
    }
    Ok(value)
}

pub(crate) fn validated_branch_name(root: &str, value: Option<&str>) -> Result<String, CoreError> {
    let value = required_text(value, "branch name")?;
    let validation = execute_git(
        root,
        &["check-ref-format".into(), "--branch".into(), value.clone()],
        None,
    )?;
    if validation.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::InvalidRequest, "Invalid Git branch name")
                .with_details(validation.output),
        );
    }
    Ok(value)
}

/// Validates a tag name against the same refname rules `git check-ref-format`
/// enforces, so an invalid name fails before any subprocess with a stable
/// message instead of depending on localized `git tag` output.
pub(crate) fn validated_tag_name(value: Option<&str>) -> Result<String, CoreError> {
    let value = required_text(value, "tag name")?;
    if is_invalid_tag_name(&value) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git tag name",
        ));
    }
    Ok(value)
}

/// Refname rules from `git check-ref-format` plus command-line safety guards
/// (no leading dash) shared by every Git mutation argument.
fn is_invalid_tag_name(value: &str) -> bool {
    if value.starts_with('-')
        || value == "@"
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("@{")
        || value.contains("//")
    {
        return true;
    }
    if value.chars().any(|character| {
        character.is_control()
            || matches!(character, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return true;
    }
    value
        .split('/')
        .any(|component| component.starts_with('.') || component.ends_with(".lock"))
}

pub(crate) fn validate_revision(value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.starts_with('-') || value.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git revision",
        ));
    }
    Ok(())
}
