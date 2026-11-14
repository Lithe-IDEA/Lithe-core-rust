//! Diffs and patches: unified and structured diffs, worktree snapshots, patch parsing
//! and `git.apply`.

use super::*;

fn append_full_file_context_arguments(arguments: &mut Vec<String>, context_lines: usize) {
    if context_lines >= FULL_FILE_CONTEXT_LINES {
        // Git xdiff combines nearby hunks using this value. Pin it to zero for
        // whole-file reviews so user configuration cannot reintroduce the
        // signed 32-bit overflow present in older Git for Windows releases.
        arguments.push("--inter-hunk-context=0".into());
    }
}

/// Builds a structured working-tree, staged, untracked, or commit diff.
pub fn diff(request: GitDiffRequest) -> Result<GitDiffResponse, CoreError> {
    let omit_file_patch = request.include_file_sections && request.omit_file_patch;
    // A complete-file review prints one hunk per file. Staging that hunk would
    // write the whole file, so Core splits it into Git-default hunks.
    let complete_file = request.include_file_sections
        && request.context_lines >= FULL_FILE_CONTEXT_LINES
        && request.stash_reference.is_none();
    let mut response = diff_impl(request)?;
    if let Some(files) = &mut response.files {
        for file in files {
            if complete_file && !file.is_binary && !file.hunks.is_empty() {
                // Fail closed: an unsplittable section offers no writable hunk.
                file.hunks = diff_blocks::stageable_hunks(&file.patch).unwrap_or_default();
                if file.status == "R" {
                    attach_rename_content_patches(&file.patch, &mut file.hunks);
                }
            }
            if omit_file_patch {
                file.patch = String::new();
            }
        }
    }
    Ok(response)
}

pub(crate) fn diff_impl(request: GitDiffRequest) -> Result<GitDiffResponse, CoreError> {
    if request.pathspecs.is_empty() || request.pathspecs.iter().any(|path| !is_safe_pathspec(path))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git diff contains an invalid path",
        ));
    }

    if request.include_file_sections
        && request.stash_reference.is_some()
        && request.pathspecs != ["."]
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Per-file stash output requires the whole repository",
        ));
    }
    let root = validate_root(&request.root)?;
    if let Some(reference) = request.stash_reference.as_deref() {
        let oid = resolve_commit_revision(&root, &validated_stash_reference(Some(reference))?)?;
        let mut arguments = vec![
            "-c".into(),
            "diff.noprefix=false".into(),
            "-c".into(),
            "diff.mnemonicprefix=false".into(),
            "stash".into(),
            "show".into(),
            "--include-untracked".into(),
            "--format=".into(),
            "--binary".into(),
            "--full-index".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--no-color".into(),
            format!("--unified={}", request.context_lines),
            oid,
        ];
        if request.include_file_sections {
            // Stash already has the safe prefix configuration at the front.
            arguments.splice(6..6, ["--numstat".into(), "-z".into(), "--patch".into()]);
        }
        let output = capture_git_with_options(&root, &arguments, None, true)?;
        if output.exit_code != 0 {
            return Err(
                CoreError::new(ErrorCode::ProcessFailed, "Could not inspect saved stash")
                    .with_details(String::from_utf8_lossy(&output.stderr).to_string()),
            );
        }
        if request.include_file_sections {
            return structured_numstat_output(output, true);
        }
        return Ok(structured_diff_from_output(output));
    }
    if request.worktree_snapshot {
        if request.reference.is_some()
            || request.empty_tree_base
            || request.git_reference.is_some()
            || request.target_git_reference.is_some()
            || request.commit.is_some()
            || request.staged
            || request.untracked
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Git worktree snapshot diff cannot combine other diff modes",
            ));
        }
        return worktree_snapshot_diff(&root, &request);
    }
    let reference = if request.empty_tree_base {
        if request.git_reference.is_some()
            || request.target_git_reference.is_some()
            || request.commit.is_some()
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Git empty-tree diff cannot combine reference forms",
            ));
        }
        let target = request.reference.as_deref().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Git empty-tree diff requires a target reference",
            )
        })?;
        validate_revision(target)?;
        Some(format!("{}..{target}", empty_tree_oid(&root)?))
    } else {
        typed_reference_range(
            &root,
            request.git_reference.as_ref(),
            request.target_git_reference.as_ref(),
            request.reference.as_deref(),
        )?
    };
    if reference.is_some() && request.commit.is_some() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git diff cannot combine a reference and a commit",
        ));
    }

    let include_untracked_with_reference = reference.is_some() && request.untracked;
    // `diff --no-index` compares one file with another, so an untracked
    // directory summary is diffed as the untracked files below it.
    let expands_untracked_directories = reference.is_none()
        && request.commit.is_none()
        && request.untracked
        && request.pathspecs.iter().any(|path| path.ends_with('/'));
    let lists_untracked_paths = include_untracked_with_reference || expands_untracked_directories;
    let mut arguments = if let Some(commit) = request.commit {
        validate_revision(&commit)?;
        vec![
            "show".to_string(),
            "--format=".to_string(),
            "--no-ext-diff".to_string(),
            "--binary".to_string(),
            format!("--unified={}", request.context_lines),
            commit,
        ]
    } else if let Some(reference) = reference {
        validate_revision(&reference)?;
        vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--binary".to_string(),
            format!("--unified={}", request.context_lines),
            reference,
        ]
    } else {
        let mut arguments = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--binary".to_string(),
        ];
        if request.untracked {
            arguments.push("--no-index".to_string());
        }
        arguments.push(format!("--unified={}", request.context_lines));
        if request.staged && !request.untracked {
            arguments.push("--cached".to_string());
        }
        arguments
    };
    append_full_file_context_arguments(&mut arguments, request.context_lines);
    if request.ignore_all_whitespace {
        arguments.push("--ignore-all-space".to_string());
    }
    arguments.push("--".to_string());
    if request.untracked && !include_untracked_with_reference {
        arguments.push(null_device().to_string());
    }
    arguments.extend(request.pathspecs.clone());

    if request.include_file_sections {
        configure_structured_diff(&mut arguments);
    }
    let root = validate_root(&request.root)?;
    let mut output = if expands_untracked_directories {
        GitProcessOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        }
    } else {
        capture_git_with_options(&root, &arguments, None, true)?
    };
    let mut sections = if request.include_file_sections {
        Some(structured_numstat_output(
            std::mem::replace(
                &mut output,
                GitProcessOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            ),
            false,
        )?)
    } else {
        None
    };
    let mut section_bytes = sections.as_ref().map(structured_file_bytes).unwrap_or(0);
    if lists_untracked_paths {
        let untracked_paths = readonly_command(GitCommandRequest {
            root: root.clone(),
            arguments: vec![
                "ls-files".into(),
                "--others".into(),
                "--exclude-standard".into(),
                "-z".into(),
                "--".into(),
            ]
            .into_iter()
            .chain(request.pathspecs.clone())
            .collect(),
            input: None,
        })?;
        if untracked_paths.exit_code != 0 {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Git untracked file lookup failed",
            )
            .with_details(untracked_paths.output));
        }
        for path in untracked_paths.stdout.split('\0') {
            if path.is_empty() {
                continue;
            }
            if !is_safe_pathspec(path) {
                // Shelve cleans a whole summarized directory after saving this
                // patch, so a file it cannot export must fail the diff rather
                // than be dropped from the patch and then deleted.
                if expands_untracked_directories {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Git diff cannot export an untracked file with an unsupported name",
                    ));
                }
                continue;
            }
            let mut untracked_arguments = vec![
                "diff".into(),
                "--no-ext-diff".into(),
                "--binary".into(),
                "--no-index".into(),
                format!("--unified={}", request.context_lines),
            ];
            append_full_file_context_arguments(&mut untracked_arguments, request.context_lines);
            if request.ignore_all_whitespace {
                untracked_arguments.push("--ignore-all-space".into());
            }
            untracked_arguments.extend(["--".into(), null_device().into(), path.into()]);
            if request.include_file_sections {
                configure_structured_diff(&mut untracked_arguments);
            }
            let untracked = capture_git_with_options(&root, &untracked_arguments, None, true)?;
            if let Some(ref mut combined) = sections {
                append_structured_files(
                    combined,
                    structured_numstat_output(untracked, false)?,
                    &mut section_bytes,
                )?;
            } else {
                output.stdout.extend(untracked.stdout);
                output.stderr.extend(untracked.stderr);
            }
        }
    }
    if let Some(sections) = sections {
        Ok(sections)
    } else {
        Ok(structured_diff_from_output(output))
    }
}

fn worktree_snapshot_diff(
    root: &str,
    request: &GitDiffRequest,
) -> Result<GitDiffResponse, CoreError> {
    let head = execute_git_readonly(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "HEAD".into(),
        ],
        None,
    )?;
    if head.exit_code > 1 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not resolve Git HEAD")
                .with_details(head.output),
        );
    }
    let head = (head.exit_code == 0).then(|| head.stdout.trim().to_string());
    let common_directory = git_resolved_path(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        "Git common directory",
    )?;
    let temporary_context = TemporaryGitCommitContext::prepare_snapshot(head.as_deref())?;
    let environment = temporary_context.environment(root, &common_directory);
    let initialize_arguments = if let Some(head) = head.as_ref() {
        vec!["read-tree".to_string(), head.clone()]
    } else {
        vec!["read-tree".to_string(), "--empty".to_string()]
    };
    let initialized =
        execute_git_with_environment(root, &initialize_arguments, None, true, &environment)?;
    if initialized.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not prepare Git snapshot diff",
        )
        .with_details(initialized.output));
    }

    let mut arguments = vec![
        "diff-files".to_string(),
        "--no-ext-diff".to_string(),
        "--binary".to_string(),
        format!("--unified={}", request.context_lines),
    ];
    append_full_file_context_arguments(&mut arguments, request.context_lines);
    if request.ignore_all_whitespace {
        arguments.push("--ignore-all-space".to_string());
    }
    arguments.push("--".to_string());
    arguments.extend(request.pathspecs.clone());
    if request.include_file_sections {
        configure_structured_diff(&mut arguments);
    }
    let mut output = capture_git_with_environment(root, &arguments, None, true, &environment)?;
    if output.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git snapshot diff failed")
                .with_details(String::from_utf8_lossy(&output.stderr)),
        );
    }

    let mut sections = if request.include_file_sections {
        Some(structured_numstat_output(
            std::mem::replace(
                &mut output,
                GitProcessOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            ),
            false,
        )?)
    } else {
        None
    };
    let mut section_bytes = sections.as_ref().map(structured_file_bytes).unwrap_or(0);
    let untracked_arguments = vec![
        "ls-files".into(),
        "--others".into(),
        "--exclude-standard".into(),
        "-z".into(),
        "--".into(),
    ]
    .into_iter()
    .chain(request.pathspecs.clone())
    .collect::<Vec<_>>();
    let untracked =
        capture_git_with_environment(root, &untracked_arguments, None, true, &environment)?;
    if untracked.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git snapshot file lookup failed")
                .with_details(String::from_utf8_lossy(&untracked.stderr)),
        );
    }
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = String::from_utf8_lossy(path).to_string();
        if !is_safe_pathspec(&path) {
            continue;
        }
        let mut untracked_arguments = vec![
            "diff".into(),
            "--no-ext-diff".into(),
            "--binary".into(),
            "--no-index".into(),
            format!("--unified={}", request.context_lines),
        ];
        append_full_file_context_arguments(&mut untracked_arguments, request.context_lines);
        if request.ignore_all_whitespace {
            untracked_arguments.push("--ignore-all-space".into());
        }
        untracked_arguments.extend(["--".into(), null_device().into(), path]);
        if request.include_file_sections {
            configure_structured_diff(&mut untracked_arguments);
        }
        let added =
            capture_git_with_environment(root, &untracked_arguments, None, true, &environment)?;
        if let Some(ref mut combined) = sections {
            append_structured_files(
                combined,
                structured_numstat_output(added, false)?,
                &mut section_bytes,
            )?;
        } else {
            output.stdout.extend(added.stdout);
            output.stderr.extend(added.stderr);
        }
    }
    if let Some(sections) = sections {
        Ok(sections)
    } else {
        Ok(structured_diff_from_output(output))
    }
}

// Request Git's NUL-delimited identities and its patch in the same process.
fn configure_structured_diff(arguments: &mut Vec<String>) {
    arguments.splice(
        1..1,
        [
            "--numstat".into(),
            "-z".into(),
            "--patch".into(),
            "--no-color".into(),
            "--no-textconv".into(),
        ],
    );
    arguments.splice(
        0..0,
        [
            "-c".into(),
            "diff.noprefix=false".into(),
            "-c".into(),
            "diff.mnemonicprefix=false".into(),
        ],
    );
}

fn structured_file_bytes(response: &GitDiffResponse) -> usize {
    response
        .files
        .as_ref()
        .map(|files| files.iter().map(|file| file.patch.len()).sum())
        .unwrap_or(0)
}

fn append_structured_files(
    target: &mut GitDiffResponse,
    source: GitDiffResponse,
    bytes: &mut usize,
) -> Result<(), CoreError> {
    *bytes = bytes
        .checked_add(structured_file_bytes(&source))
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Git diff exceeds 32 MiB"))?;
    if *bytes > patch_exchange::MAX_PATCH_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git diff exceeds 32 MiB",
        ));
    }
    target.has_decoding_loss |= source.has_decoding_loss;
    if let (Some(target), Some(source)) = (&mut target.files, source.files) {
        target.extend(source);
    }
    Ok(())
}

pub(crate) fn structured_diff_from_output(output: GitProcessOutput) -> GitDiffResponse {
    // Diff is a machine-readable stdout protocol. Git diagnostics on stderr
    // must never become synthetic file lines in the parsed patch.
    let decoded = String::from_utf8_lossy(&output.stdout);
    // A Cow::Owned means invalid UTF-8 bytes were replaced with U+FFFD; the
    // patch cannot be used as byte-faithful write material.
    let has_decoding_loss = matches!(decoded, std::borrow::Cow::Owned(_));
    let patch = decoded.to_string();
    let document = parse_diff(&patch);
    GitDiffResponse {
        patch,
        rows: document.0,
        hunks: document.1,
        has_decoding_loss,
        files: None,
    }
}

/// Adds a content-only patch to every hunk of a rename section, so applying
/// one hunk changes the target file's text without reversing the rename.
fn attach_rename_content_patches(section: &str, hunks: &mut [GitDiffHunkResponse]) {
    let target = section
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .find(|line| line.starts_with("+++ b/") || line.starts_with("+++ \"b/"));
    let Some(target) = target else {
        return;
    };
    let old = target
        .strip_prefix("+++ b/")
        .map(|path| format!("--- a/{path}"))
        .or_else(|| {
            target
                .strip_prefix("+++ \"b/")
                .map(|path| format!("--- \"a/{path}"))
        });
    let Some(old) = old else {
        return;
    };
    for hunk in hunks {
        let marker = format!("\n{}", hunk.header);
        if let Some(offset) = hunk.patch.find(&marker) {
            hunk.content_patch = Some(format!("{old}\n{target}\n{}", &hunk.patch[offset + 1..]));
        }
    }
}

// Git owns filename decoding. Pair its ordered NUL-delimited metadata with
// byte-preserving section boundaries, then parse each document once.
fn structured_patch_sections(
    patch: &str,
    identities: Vec<patch_exchange::PatchFile>,
    has_decoding_loss: bool,
) -> Result<GitDiffResponse, CoreError> {
    let mut files = Vec::new();
    if !patch.is_empty() {
        let mut starts = Vec::new();
        let mut offset = 0;
        for line in patch.split_inclusive('\n') {
            if line.starts_with("diff --git ") {
                starts.push(offset);
            }
            offset += line.len();
        }
        if starts.first() != Some(&0) || starts.len() != identities.len() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Patch sections do not match Git metadata",
            ));
        }
        starts.push(patch.len());
        for (identity, range) in identities.into_iter().zip(starts.windows(2)) {
            let section = &patch[range[0]..range[1]];
            let status = if section
                .lines()
                .any(|line| line.starts_with("new file mode "))
            {
                "A"
            } else if section
                .lines()
                .any(|line| line.starts_with("deleted file mode "))
            {
                "D"
            } else if identity.original_path.is_some() {
                "R"
            } else {
                "M"
            };
            let is_binary = identity.additions.is_none() && identity.deletions.is_none();
            let has_lossy_line_endings = has_decoding_loss
                || section.split('\n').any(|line| {
                    line.ends_with('\r') || line.starts_with("\\ No newline at end of file")
                });
            let (rows, mut hunks, lines) = parse_diff(section);
            if status == "R" {
                attach_rename_content_patches(section, &mut hunks);
            }
            files.push(GitDiffFileResponse {
                path: identity.path,
                original_path: identity.original_path,
                status: status.into(),
                patch: section.into(),
                rows,
                hunks,
                lines,
                is_binary,
                has_lossy_line_endings,
                additions: identity.additions,
                deletions: identity.deletions,
            });
        }
    } else if !identities.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Patch sections do not match Git metadata",
        ));
    }
    Ok(GitDiffResponse {
        patch: String::new(),
        rows: Vec::new(),
        hunks: Vec::new(),
        has_decoding_loss,
        files: Some(files),
    })
}

// Git writes all `--numstat -z` records first, followed by one extra NUL and
// the patch. NUL cannot occur inside a Git path, including quoted path names.
pub(crate) fn structured_numstat_output(
    output: GitProcessOutput,
    require_utf8: bool,
) -> Result<GitDiffResponse, CoreError> {
    if output.stdout.is_empty() {
        return structured_patch_sections("", Vec::new(), false);
    }
    let separator = output
        .stdout
        .windows(2)
        .position(|bytes| bytes == [0, 0])
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Git diff metadata is incomplete")
        })?;
    let identities = patch_exchange::parse_diff_statistics_in_order(&output.stdout[..=separator])?;
    let bytes = &output.stdout[separator + 2..];
    if bytes.len() > patch_exchange::MAX_PATCH_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Saved patch exceeds 32 MiB",
        ));
    }
    let decoded = String::from_utf8_lossy(bytes);
    let has_decoding_loss = matches!(decoded, std::borrow::Cow::Owned(_));
    if require_utf8 && has_decoding_loss {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Saved patch must contain valid UTF-8",
        ));
    }
    structured_patch_sections(&decoded, identities, has_decoding_loss)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParsePatchRequest {
    root: String,
    patch: String,
}

/// Handles `git.parsePatch`: parses a saved patch into validated per-file documents.
pub fn parse_patch_command(payload: serde_json::Value) -> Result<serde_json::Value, CoreError> {
    let request: ParsePatchRequest = serde_json::from_value(payload).map_err(|_| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid Git patch parse request")
    })?;
    let root = validate_root(&request.root)?;
    if request.patch.len() > patch_exchange::MAX_PATCH_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Saved patch exceeds 32 MiB",
        ));
    }
    let identities = if request.patch.is_empty() {
        Vec::new()
    } else {
        patch_exchange::ordered_patch_files(&root, &request.patch)?
    };
    serde_json::to_value(structured_patch_sections(
        &request.patch,
        identities,
        false,
    )?)
    .map_err(|_| CoreError::new(ErrorCode::Unknown, "Could not encode parsed Git patch"))
}

/// Applies a validated patch using the requested index or working-tree mode.
pub fn apply(request: GitApplyRequest) -> Result<GitCommandResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let _lease = rewrite::RewriteLease::acquire(&root)?;
    let arguments = match request.mode.as_str() {
        "stage" => vec![
            "apply".to_string(),
            "--cached".to_string(),
            "--whitespace=nowarn".to_string(),
        ],
        "unstage" => vec![
            "apply".to_string(),
            "--cached".to_string(),
            "--reverse".to_string(),
            "--whitespace=nowarn".to_string(),
        ],
        "discard" => {
            // A hunk rollback rewrites worktree bytes; a clean/smudge filter or
            // working-tree encoding could change bytes the patch never saw.
            let mut paths = Vec::new();
            for file in patch_exchange::ordered_patch_files(&root, &request.patch)? {
                paths.extend(file.original_path);
                paths.push(file.path);
            }
            if !diff_blocks::worktree_attributes_safe(&root, &paths)? {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    diff_blocks::HUNK_ROLLBACK_UNSAFE,
                ));
            }
            vec![
                "apply".to_string(),
                "--reverse".to_string(),
                "--whitespace=nowarn".to_string(),
            ]
        }
        // Reconstruct a saved index snapshot and its worktree in one step.
        // `--cached` alone would leave the worktree at HEAD, which makes a
        // subsequent unstaged patch fail for files with both staged and
        // unstaged edits.
        "restoreIndex" => vec![
            "apply".to_string(),
            "--index".to_string(),
            "--whitespace=nowarn".to_string(),
        ],
        "worktree" => vec!["apply".to_string(), "--whitespace=nowarn".to_string()],
        "restoreIndexCheck" => vec![
            "apply".to_string(),
            "--cached".to_string(),
            "--reverse".to_string(),
            "--check".to_string(),
            "--whitespace=nowarn".to_string(),
        ],
        "worktreeCheck" => vec![
            "apply".to_string(),
            "--reverse".to_string(),
            "--check".to_string(),
            "--whitespace=nowarn".to_string(),
        ],
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unsupported Git patch mode",
            ))
        }
    };
    command(GitCommandRequest {
        root: request.root,
        arguments,
        input: Some(request.patch),
    })
}

/// One removed or added line retained with its original side's line number.
struct DiffEntry {
    number: usize,
    text: String,
}

/// Parsed patch hunk before it is aligned into side-by-side rows.
struct DiffHunkRecord {
    id: String,
    header: String,
    lines: Vec<String>,
}

fn without_one_trailing_cr(value: &str) -> &str {
    value.strip_suffix('\r').unwrap_or(value)
}

fn flush_diff_changes(
    rows: &mut Vec<GitDiffRowResponse>,
    removed: &mut Vec<DiffEntry>,
    added: &mut Vec<DiffEntry>,
    hunk_id: Option<&str>,
) {
    let removed_text: Vec<&str> = removed.iter().map(|entry| entry.text.as_str()).collect();
    let added_text: Vec<&str> = added.iter().map(|entry| entry.text.as_str()).collect();
    for (left_index, right_index) in lithe_diff::pair_lines(&removed_text, &added_text) {
        let left = left_index.map(|index| &removed[index]);
        let right = right_index.map(|index| &added[index]);
        let kind = match (left.is_some(), right.is_some()) {
            (true, true) => "changed",
            (true, false) => "removal",
            (false, true) => "addition",
            (false, false) => continue,
        };
        let is_invisible_change = left.zip(right).and_then(|(left, right)| {
            (without_one_trailing_cr(&left.text) == without_one_trailing_cr(&right.text))
                .then_some(true)
        });
        let (left_changes, right_changes) = match left.zip(right) {
            Some((left, right)) if is_invisible_change.is_none() => {
                match lithe_diff::inline_changes(
                    without_one_trailing_cr(&left.text),
                    without_one_trailing_cr(&right.text),
                ) {
                    Some((left, right)) => (Some(left), Some(right)),
                    None => (None, None),
                }
            }
            _ => (None, None),
        };
        rows.push(GitDiffRowResponse {
            old_line: left.map(|entry| entry.number),
            new_line: right.map(|entry| entry.number),
            left: left.map(|entry| entry.text.clone()),
            right: right.map(|entry| entry.text.clone()),
            kind: kind.to_string(),
            hunk_id: hunk_id.map(String::from),
            is_invisible_change,
            left_changes,
            right_changes,
        });
    }
    removed.clear();
    added.clear();
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize)> {
    let mut columns = header.split_whitespace();
    if columns.next()? != "@@" {
        return None;
    }
    let old_range = columns.next()?.strip_prefix('-')?;
    let new_range = columns.next()?.strip_prefix('+')?;
    let old_line = old_range.split(',').next()?.parse().ok()?;
    let new_line = new_range.split(',').next()?.parse().ok()?;
    Some((old_line, new_line))
}

pub(crate) fn parse_diff(
    patch: &str,
) -> (
    Vec<GitDiffRowResponse>,
    Vec<GitDiffHunkResponse>,
    Vec<GitDiffUnifiedLineResponse>,
) {
    let has_trailing_newline = patch.ends_with('\n');
    let lines = patch.strip_suffix('\n').unwrap_or(patch).split('\n');

    let mut rows = Vec::new();
    let mut unified = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut current_hunk_id: Option<String> = None;
    let mut current_hunk_header = String::new();
    let mut current_hunk_lines: Vec<String> = Vec::new();
    let mut file_header_lines: Vec<String> = Vec::new();
    let mut hunk_records = Vec::new();
    let mut hunk_index = 0;

    for line in lines {
        if line.starts_with("@@") {
            if let Some(hunk_id) = current_hunk_id.take() {
                flush_diff_changes(&mut rows, &mut removed, &mut added, Some(&hunk_id));
                hunk_records.push(DiffHunkRecord {
                    id: hunk_id,
                    header: std::mem::take(&mut current_hunk_header),
                    lines: std::mem::take(&mut current_hunk_lines),
                });
            }

            let hunk_id = format!("hunk-{hunk_index}");
            hunk_index += 1;
            current_hunk_id = Some(hunk_id.clone());
            current_hunk_header = line.to_string();
            current_hunk_lines = file_header_lines.clone();
            current_hunk_lines.push(line.to_string());
            if let Some((old, new)) = parse_hunk_header(line) {
                old_line = old;
                new_line = new;
            }
            unified.push(GitDiffUnifiedLineResponse {
                line_type: "header".into(),
                content: without_one_trailing_cr(line).into(),
                old_line: None,
                new_line: None,
            });
            rows.push(GitDiffRowResponse {
                old_line: None,
                new_line: None,
                left: Some(line.to_string()),
                right: None,
                kind: "information".to_string(),
                hunk_id: Some(hunk_id),
                is_invisible_change: None,
                left_changes: None,
                right_changes: None,
            });
        } else if line.starts_with("diff --git") && current_hunk_id.is_some() {
            let hunk_id = current_hunk_id.take().expect("hunk should exist");
            flush_diff_changes(&mut rows, &mut removed, &mut added, Some(&hunk_id));
            hunk_records.push(DiffHunkRecord {
                id: hunk_id,
                header: std::mem::take(&mut current_hunk_header),
                lines: std::mem::take(&mut current_hunk_lines),
            });
            file_header_lines = vec![line.to_string()];
        } else if current_hunk_id.is_none() {
            file_header_lines.push(line.to_string());
        } else if let Some(rest) = line.strip_prefix('-') {
            unified.push(GitDiffUnifiedLineResponse {
                line_type: "removed".into(),
                content: without_one_trailing_cr(rest).into(),
                old_line: Some(old_line),
                new_line: None,
            });
            current_hunk_lines.push(line.to_string());
            removed.push(DiffEntry {
                number: old_line,
                text: rest.to_string(),
            });
            old_line += 1;
        } else if let Some(rest) = line.strip_prefix('+') {
            unified.push(GitDiffUnifiedLineResponse {
                line_type: "added".into(),
                content: without_one_trailing_cr(rest).into(),
                old_line: None,
                new_line: Some(new_line),
            });
            current_hunk_lines.push(line.to_string());
            added.push(DiffEntry {
                number: new_line,
                text: rest.to_string(),
            });
            new_line += 1;
        } else if let Some(rest) = line.strip_prefix(' ') {
            unified.push(GitDiffUnifiedLineResponse {
                line_type: "context".into(),
                content: without_one_trailing_cr(rest).into(),
                old_line: Some(old_line),
                new_line: Some(new_line),
            });
            let hunk_id = current_hunk_id.as_deref();
            flush_diff_changes(&mut rows, &mut removed, &mut added, hunk_id);
            current_hunk_lines.push(line.to_string());
            rows.push(GitDiffRowResponse {
                old_line: Some(old_line),
                new_line: Some(new_line),
                left: Some(rest.to_string()),
                right: None,
                kind: "context".to_string(),
                hunk_id: current_hunk_id.clone(),
                is_invisible_change: None,
                left_changes: None,
                right_changes: None,
            });
            old_line += 1;
            new_line += 1;
        } else {
            current_hunk_lines.push(line.to_string());
        }
    }

    if let Some(hunk_id) = current_hunk_id.take() {
        flush_diff_changes(&mut rows, &mut removed, &mut added, Some(&hunk_id));
        hunk_records.push(DiffHunkRecord {
            id: hunk_id,
            header: current_hunk_header,
            lines: current_hunk_lines,
        });
    }

    let hunks = hunk_records
        .into_iter()
        .map(|record| {
            let patch = record.lines.join("\n") + if has_trailing_newline { "\n" } else { "" };
            GitDiffHunkResponse {
                id: record.id,
                header: record.header,
                patch,
                content_patch: None,
                line_start: None,
                line_end: None,
            }
        })
        .collect();
    (rows, hunks, unified)
}
