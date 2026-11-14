use super::{
    annotation_message_from_tag_object, parse_commit, parse_commit_lookup, parse_diff,
    pathspec_batches, simplified_canonical_path, structured_diff_from_output, GitCommandInvocation,
    GitCommandResponse, GitProcessOutput,
};
use crate::contracts::{
    GitCommitLookupResponse, GitCommitResponse, GitHistoryPageResponse, GitHistoryResponse,
    GitPushPreviewResponse, GitPushTagResponse, GitPushTargetKind, GitReferenceResponse,
    GitReferencesResponse,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde_json::Value;
use std::path::PathBuf;

/// Only an exact `Name <email>` names an author; the options keep Git's order.
#[test]
fn commit_identity_requires_an_exact_author() {
    assert_eq!(
        super::commit_identity(Some("张三 <zhang@example.invalid>"), true).unwrap(),
        ["--author=张三 <zhang@example.invalid>", "--signoff"]
    );
    assert!(super::commit_identity(None, false).unwrap().is_empty());
    for author in ["", " A <a@x>", "A  <a b@x>", "A <a@x> ", "A <>", "A\t<a@x>"] {
        let error = super::commit_identity(Some(author), false).unwrap_err();
        assert!(
            matches!(error.code, ErrorCode::GitCommitAuthorInvalid),
            "{author:?}"
        );
    }
}

#[test]
fn unforced_removal_refused_for_local_changes_gains_its_own_code() {
    let failed = |stderr: &str| {
        GitProcessOutput {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code: 128,
        }
        .into_command_response(&["worktree".to_string(), "remove".to_string()])
    };
    let mut dirty = failed(
        "fatal: '/tmp/feature' contains modified or untracked files, use --force to delete it\n",
    );
    super::classify_unclean_worktree_removal(&mut dirty);
    let error = dirty.operation_error.expect("operation error");
    assert!(matches!(error.code, ErrorCode::GitWorktreeHasChanges));
    assert_eq!(
        error.message,
        "fatal: '/tmp/feature' contains modified or untracked files, use --force to delete it"
    );
    assert_eq!(
        serde_json::to_value(CoreError::new(ErrorCode::GitWorktreeHasChanges, "x")).unwrap()
            ["code"],
        "git_worktree_has_changes"
    );

    // Submodules also need --force, but for a different reason; that refusal
    // keeps Git's plain failure rather than claiming local changes.
    let mut submodules =
        failed("fatal: working trees containing submodules cannot be moved or removed\n");
    super::classify_unclean_worktree_removal(&mut submodules);
    assert!(submodules.operation_error.is_none());

    // A more specific error that is already present is kept.
    let mut specific = failed("fatal: '/tmp/feature' contains modified or untracked files\n");
    specific.operation_error = Some(CoreError::new(ErrorCode::Cancelled, "cancelled"));
    super::classify_unclean_worktree_removal(&mut specific);
    assert!(matches!(
        specific.operation_error.unwrap().code,
        ErrorCode::Cancelled
    ));

    let mut removed = failed("");
    removed.exit_code = 0;
    super::classify_unclean_worktree_removal(&mut removed);
    assert!(removed.operation_error.is_none());
}

#[test]
fn saved_stash_sections_reject_lossy_or_oversized_patch_before_git() {
    let output = |stdout| super::GitProcessOutput {
        stdout,
        stderr: Vec::new(),
        exit_code: 0,
    };
    let invalid = super::structured_numstat_output(
        output(vec![b'1', b'\t', b'0', b'\t', b'a', 0, 0, 0xff]),
        true,
    )
    .unwrap_err();
    assert_eq!(invalid.message, "Saved patch must contain valid UTF-8");
    let oversized = super::structured_numstat_output(
        output(
            [
                b"1\t0\ta\0\0".as_slice(),
                &vec![b'x'; super::patch_exchange::MAX_PATCH_BYTES + 1],
            ]
            .concat(),
        ),
        true,
    )
    .unwrap_err();
    assert_eq!(oversized.message, "Saved patch exceeds 32 MiB");
    let empty = super::structured_numstat_output(output(Vec::new()), true).unwrap();
    assert!(empty.files.unwrap().is_empty());
}

#[test]
fn temporary_git_context_is_private_and_cleans_up_under_a_public_parent() {
    let parent = super::TemporaryGitCommitContext::prepare_snapshot(Some(&"a".repeat(40)))
        .expect("owned fixture parent should be created");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&parent.directory, std::fs::Permissions::from_mode(0o755))
            .expect("fixture parent should allow public traversal");
    }
    let context = super::TemporaryGitCommitContext::prepare_in(
        &parent.directory,
        "private-index",
        Some(&"b".repeat(40)),
    )
    .expect("private context should be created atomically");
    let directory = context.directory.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    std::fs::write(&context.index_path, b"tracked/path\0object-id")
        .expect("index fixture should stay inside the owned context");
    assert_eq!(
        std::fs::read(directory.join("HEAD")).unwrap(),
        format!("{}\n", "b".repeat(40)).as_bytes()
    );
    drop(context);
    assert!(
        !directory.exists(),
        "HEAD and index should be removed with their owner"
    );
}

#[test]
fn simplified_canonical_path_strips_windows_verbatim_prefixes() {
    // Windows canonicalization yields verbatim paths. Consumers normalize
    // separators, which would turn them into `//?/C:/...` and break every
    // later lookup of the discovered repository root.
    assert_eq!(
        simplified_canonical_path(PathBuf::from(r"\\?\C:\work\repo")).unwrap(),
        PathBuf::from(r"C:\work\repo")
    );
    assert_eq!(
        simplified_canonical_path(PathBuf::from(r"\\?\UNC\server\share\repo")).unwrap(),
        PathBuf::from(r"\\server\share\repo")
    );
    assert_eq!(
        simplified_canonical_path(PathBuf::from("/work/repo")).unwrap(),
        PathBuf::from("/work/repo")
    );
}

#[test]
fn windows_git_paths_follow_shared_identity_policy() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../fixtures/git/windows-paths.json")).unwrap();
    for case in cases["accepted"].as_array().unwrap() {
        let path =
            simplified_canonical_path(PathBuf::from(case["input"].as_str().unwrap())).unwrap();
        assert_eq!(
            path.to_string_lossy().replace('\\', "/"),
            case["normalized"].as_str().unwrap()
        );
    }
    for path in cases["rejected"].as_array().unwrap() {
        let path = PathBuf::from(path.as_str().unwrap());
        let error = simplified_canonical_path(path.clone()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{path:?}");
        // A missing/stale worktree must not bypass the same identity check.
        assert!(super::canonicalize_or_original(&path).is_err());
    }
}

#[test]
fn repository_root_discovery_does_not_acquire_a_write_lease() {
    struct Repository(std::path::PathBuf);
    impl Drop for Repository {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("temporary repository should be removed");
        }
    }

    let repository = Repository(std::env::temp_dir().join(format!(
        "lithe-repository-root-{}-{}",
        std::process::id(),
        super::TEMPORARY_INDEX_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&repository.0).unwrap();
    let root = repository.0.to_string_lossy().into_owned();
    let initialized = super::command(super::GitCommandRequest {
        root: root.clone(),
        arguments: vec!["init".into(), "-q".into()],
        input: None,
    })
    .unwrap();
    assert_eq!(initialized.exit_code, 0);

    let lease = super::rewrite::RewriteLease::acquire(&root).unwrap();
    let response = crate::test_envelope(
        crate::test_request(serde_json::json!({ "root": root.clone() }))
            .and_then(super::discover_repository_root),
    );
    drop(lease);

    let expected_root = super::canonicalize_simplified(&repository.0)
        .expect("temporary repository should canonicalize")
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["data"],
        serde_json::json!(expected_root),
        "{response}"
    );
}

#[test]
fn argument_and_typed_writes_reject_an_active_repository_lease() {
    struct Repository(std::path::PathBuf);
    impl Drop for Repository {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("temporary repository should be removed");
        }
    }

    let repository = Repository(std::env::temp_dir().join(format!(
        "lithe-write-lease-{}-{}",
        std::process::id(),
        super::TEMPORARY_INDEX_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&repository.0).unwrap();
    let root = repository.0.to_string_lossy().into_owned();
    let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(5_000));
    let initialized = super::command(super::GitCommandRequest {
        root: root.clone(),
        arguments: vec!["init".into(), "-q".into()],
        input: None,
    })
    .unwrap();
    assert_eq!(initialized.exit_code, 0);

    let lease = super::rewrite::RewriteLease::acquire(&root).unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let run =
            |payload: Value, operation: fn(Value) -> Result<GitCommandResponse, CoreError>| {
                let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(2_000));
                let mut payload = payload;
                payload["root"] = serde_json::json!(root);
                crate::test_envelope(operation(payload))
            };
        let results = [
            run(
                serde_json::json!({"arguments":["config", "test.writer", "raw"]}),
                |payload| crate::test_request(payload).and_then(super::command),
            ),
            run(serde_json::json!({"operation":"stageAll"}), |payload| {
                crate::test_request(payload).and_then(super::write)
            }),
        ];
        let _ = completed.send(results);
    });

    // The competing commands must finish while the writer still owns its
    // lease. Always release it before asserting, so a blocking regression
    // can terminate and be joined even when the first deadline is missed.
    let results = completion.recv_timeout(std::time::Duration::from_secs(5));
    drop(lease);
    if results.is_err() {
        completion
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("competing writer should terminate after lease cleanup");
    }
    worker.join().expect("competing writer should not panic");
    for result in results.expect("competing writers must fail without waiting for the lease") {
        assert_eq!(result["ok"], false, "{result}");
        assert_eq!(result["error"]["code"], "invalid_request", "{result}");
        assert_eq!(
            result["error"]["message"],
            "Another Git write operation is running in this repository"
        );
    }
}

#[test]
fn tag_annotation_parser_preserves_crlf_and_trailing_blank_lines() {
    let raw = concat!(
        "object abc123\n",
        "type commit\n",
        "tag v1.0\n",
        "tagger Lithe Test <test@example.com> 0 +0000\n",
        "\n",
        "release\r\n",
        "\r\n",
        "details\r\n",
        "\r\n"
    );

    assert_eq!(
        annotation_message_from_tag_object(raw).as_deref(),
        Some("release\r\n\r\ndetails\r\n\r\n")
    );
}

#[test]
fn tag_annotation_parser_removes_only_the_signature_block() {
    let raw = concat!(
        "object abc123\n",
        "type commit\n",
        "tag v1.0\n",
        "tagger Lithe Test <test@example.com> 0 +0000\n",
        "\n",
        "release\r\n",
        "\r\n",
        "-----BEGIN PGP SIGNATURE-----\r\n",
        "signature-data\r\n",
        "-----END PGP SIGNATURE-----\r\n"
    );

    assert_eq!(
        annotation_message_from_tag_object(raw).as_deref(),
        Some("release\r\n\r\n")
    );
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagWriteFixture {
    target: String,
    cases: Vec<TagWriteArgumentCase>,
    rejected_cases: Vec<TagWriteRejectedCase>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagWriteArgumentCase {
    name: String,
    request: TagWriteFields,
    arguments: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagWriteFields {
    message: Option<String>,
    lightweight: Option<bool>,
    signed: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagWriteRejectedCase {
    name: String,
    request: TagWriteFields,
    error: TagWriteRejectedError,
}

#[derive(serde::Deserialize)]
struct TagWriteRejectedError {
    code: String,
    message: String,
}

#[test]
fn create_tag_arguments_follow_the_lightweight_and_signature_matrix() {
    // The fixture pins the argument matrix shared with the Windows host,
    // including the combinations that must fail before any subprocess.
    let fixture: TagWriteFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/tag-write-v1.json"
    )))
    .expect("tag write fixture should be valid JSON");
    for case in &fixture.cases {
        super::validate_create_tag_form(
            case.request.message.as_deref(),
            case.request.lightweight,
            case.request.signed,
        )
        .unwrap_or_else(|error| panic!("{}: {}", case.name, error.message));
        assert_eq!(
            super::create_tag_arguments(
                "v1.0",
                &fixture.target,
                case.request.message.as_deref(),
                case.request.lightweight,
                case.request.signed,
            ),
            case.arguments,
            "{}",
            case.name
        );
    }
    for case in &fixture.rejected_cases {
        let error = super::validate_create_tag_form(
            case.request.message.as_deref(),
            case.request.lightweight,
            case.request.signed,
        )
        .expect_err(&case.name);
        let encoded = serde_json::to_value(&error).expect("tag form error should encode");
        assert_eq!(encoded["code"], case.error.code, "{}", case.name);
        assert_eq!(error.message, case.error.message, "{}", case.name);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn background_git_processes_do_not_create_windows_console() {
    assert_eq!(super::git_process_creation_flags(), 0x08000000);
}

// A Latin-1 `é` (0xE9) decodes to U+FFFD; writing that patch back would
// replace the original byte with EF BF BD, so the response must say so.
#[test]
fn structured_diff_reports_lossy_utf8_decoding() {
    let lossy = structured_diff_from_output(GitProcessOutput {
        stdout: b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-caf\xe9\r\n+cafe\r\n".to_vec(),
        stderr: Vec::new(),
        exit_code: 1,
    });
    assert!(lossy.has_decoding_loss);
    assert!(lossy.patch.contains('\u{FFFD}'));

    let faithful = structured_diff_from_output(GitProcessOutput {
        stdout:
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-café\r\n+cafe\r\n"
                .as_bytes()
                .to_vec(),
        stderr: Vec::new(),
        exit_code: 1,
    });
    assert!(!faithful.has_decoding_loss);
    assert!(faithful.patch.contains("café\r\n"));
}

#[test]
fn structured_diff_does_not_parse_git_stderr_as_file_content() {
    let output = GitProcessOutput {
        stdout: b"diff --git a/c.txt b/c.txt\n--- /dev/null\n+++ b/c.txt\n@@ -0,0 +1,3 @@\n+c\n+cc\n+ccc\n".to_vec(),
        stderr: b"warning: CRLF will be replaced by LF\n".to_vec(),
        exit_code: 1,
    };

    let diff = structured_diff_from_output(output);

    assert!(!diff.patch.contains("warning:"));
    assert!(diff
        .rows
        .iter()
        .any(|row| row.right.as_deref() == Some("ccc")));
    assert!(!diff.rows.iter().any(|row| row
        .right
        .as_deref()
        .is_some_and(|line| line.contains("warning:"))));
}

#[test]
fn traced_response_uses_the_final_invocation_for_compatibility_fields() {
    let mut response = GitCommandResponse {
        arguments: vec!["status".into()],
        output: "stale".into(),
        stdout: "stale".into(),
        stderr: String::new(),
        exit_code: 0,
        invocations: vec![
            GitCommandInvocation {
                arguments: vec!["status".into()],
                stdout: "status\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
            GitCommandInvocation {
                arguments: vec!["stash".into(), "list".into()],
                stdout: "stash@{0}\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
        ],
        operation_error: None,
        stash_restore: None,
        tag_deletion: None,
        branch_deletion: None,
        history_rewrite: None,
        outcome_authoritative: false,
        warnings: Vec::new(),
    };

    super::synchronize_final_invocation(&mut response);

    assert_eq!(response.arguments, vec!["stash", "list"]);
    assert_eq!(response.stdout, "stash@{0}\n");
    assert_eq!(response.stderr, "");
    assert_eq!(response.output, "stash@{0}\n");
    assert_eq!(response.exit_code, 0);
}

#[test]
fn traced_core_error_becomes_a_failed_response_with_invocations() {
    let result = super::with_git_invocation_trace(|| {
        let response = GitProcessOutput {
            stdout: b"prepared\n".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        }
        .into_command_response(&["stash".into(), "push".into()]);
        super::record_git_invocation(&response);
        Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Follow-up probe failed",
        ))
    });

    let response = result.expect("a partial Git failure should retain command data");
    assert_eq!(response.exit_code, 0);
    assert_eq!(response.arguments, vec!["stash", "push"]);
    assert_eq!(response.output, "prepared\n");
    assert_eq!(response.invocations.len(), 1);
    assert_eq!(
        response
            .operation_error
            .as_ref()
            .map(|error| &error.message),
        Some(&"Follow-up probe failed".to_string())
    );
}

#[test]
fn git_process_response_preserves_executed_arguments() {
    let arguments = vec!["status".to_string(), "--short".to_string()];
    let response = GitProcessOutput {
        stdout: b" M README.md\n".to_vec(),
        stderr: Vec::new(),
        exit_code: 0,
    }
    .into_command_response(&arguments);

    assert_eq!(response.arguments, arguments);
    assert_eq!(response.output, " M README.md\n");
    assert_eq!(response.stdout, " M README.md\n");
    assert_eq!(response.stderr, "");
    assert_eq!(response.exit_code, 0);
}

#[test]
fn command_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/command-response-v1.json"
    )))
    .expect("Git command response fixture should be valid JSON");
    let response = GitCommandResponse {
        arguments: vec![
            "checkout".into(),
            "HEAD".into(),
            "--".into(),
            "README.md".into(),
        ],
        output: String::new(),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        invocations: vec![
            GitCommandInvocation {
                arguments: vec![
                    "status".into(),
                    "--porcelain".into(),
                    "--untracked-files=all".into(),
                    "--".into(),
                    "README.md".into(),
                ],
                stdout: " M README.md\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
            GitCommandInvocation {
                arguments: vec![
                    "checkout".into(),
                    "HEAD".into(),
                    "--".into(),
                    "README.md".into(),
                ],
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            },
        ],
        operation_error: None,
        stash_restore: None,
        tag_deletion: None,
        branch_deletion: None,
        history_rewrite: None,
        outcome_authoritative: false,
        warnings: Vec::new(),
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git response should serialize"),
        fixture
    );
}

#[test]
fn parse_commit_splits_the_author_utc_offset_from_the_displayed_date() {
    let line = |date: &str| format!("h\u{1f}s\u{1f}\u{1f}A\u{1f}a@x\u{1f}{date}\u{1f}msg\u{1f}");

    let east = parse_commit(&line("2026/09/30 21:05 +0800")).expect("commit should parse");
    assert_eq!(east.date, "2026/09/30 21:05");
    assert_eq!(east.date_utc_offset_minutes, Some(480));
    assert_eq!(east.date_unix_seconds, Some(1790773500));

    let west = parse_commit(&line("2026/09/30 08:05 -0330")).expect("commit should parse");
    assert_eq!(west.date, "2026/09/30 08:05");
    assert_eq!(west.date_utc_offset_minutes, Some(-210));

    // Output without a recognizable zone stays visible unchanged instead of being trimmed.
    let legacy = parse_commit(&line("2026/09/30 08:05")).expect("commit should parse");
    let invalid = parse_commit(&line("2026/09/30 08:05 +1260")).expect("commit should parse");
    assert_eq!(invalid.date, "2026/09/30 08:05 +1260");
    assert_eq!(invalid.date_utc_offset_minutes, None);
    assert_eq!(invalid.date_unix_seconds, None);

    assert_eq!(legacy.date, "2026/09/30 08:05");
    assert_eq!(legacy.date_utc_offset_minutes, None);
}

#[test]
fn history_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/history-response-v1.json"
    )))
    .expect("Git history response fixture should be valid JSON");
    let feature = GitReferenceResponse {
        full_name: "refs/heads/feature/recent".into(),
        short_name: "feature/recent".into(),
        kind: "local".into(),
        peels_to_commit: true,
        is_current: true,
        upstream_short_name: None,
        ahead: 0,
        behind: 0,
    };
    let main = GitReferenceResponse {
        full_name: "refs/heads/main".into(),
        short_name: "main".into(),
        kind: "local".into(),
        peels_to_commit: true,
        is_current: false,
        upstream_short_name: Some("origin/main".into()),
        ahead: 2,
        behind: 1,
    };
    let response = GitHistoryResponse {
        references: vec![feature.clone(), main.clone()],
        recent_references: vec![feature, main],
        commits: vec![GitCommitResponse {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            short_hash: "0123456".into(),
            parent_hashes: Vec::new(),
            author_name: "Lithe Test".into(),
            author_email: "test@example.invalid".into(),
            date: "2026/08/30 12:00".into(),
            date_utc_offset_minutes: Some(480),
            date_unix_seconds: None,
            subject: "Initial commit".into(),
            decorations: "HEAD -> feature/recent".into(),
        }],
        has_more: false,
        user_name: Some("Lithe Test".into()),
        user_email: Some("test@example.invalid".into()),
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git history response should serialize"),
        fixture
    );
}

#[test]
fn references_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/references-response-v1.json"
    )))
    .expect("Git references response fixture should be valid JSON");
    let feature = GitReferenceResponse {
        full_name: "refs/heads/feature/recent".into(),
        short_name: "feature/recent".into(),
        kind: "local".into(),
        peels_to_commit: true,
        is_current: true,
        upstream_short_name: None,
        ahead: 0,
        behind: 0,
    };
    let main = GitReferenceResponse {
        full_name: "refs/heads/main".into(),
        short_name: "main".into(),
        kind: "local".into(),
        peels_to_commit: true,
        is_current: false,
        upstream_short_name: Some("origin/main".into()),
        ahead: 2,
        behind: 1,
    };
    let response = GitReferencesResponse {
        references: vec![feature.clone(), main.clone()],
        recent_references: vec![feature, main],
        user_name: Some("Lithe Test".into()),
        user_email: Some("test@example.invalid".into()),
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git references response should serialize"),
        fixture
    );
}

#[test]
fn history_page_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/history-page-response-v1.json"
    )))
    .expect("Git history page response fixture should be valid JSON");
    let response = GitHistoryPageResponse {
        commits: vec![GitCommitResponse {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            short_hash: "0123456".into(),
            parent_hashes: Vec::new(),
            author_name: "Lithe Test".into(),
            author_email: "test@example.invalid".into(),
            date: "2026/08/30 12:00".into(),
            date_utc_offset_minutes: Some(480),
            date_unix_seconds: None,
            subject: "Initial commit".into(),
            decorations: "HEAD -> feature/recent".into(),
        }],
        next_cursor: Some("git-history-cursor-fixture".into()),
        next_offset: None,
        has_more: true,
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git history page response should serialize"),
        fixture
    );
}

#[test]
fn commit_lookup_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/commit-lookup-response-v1.json"
    )))
    .expect("Git commit lookup response fixture should be valid JSON");
    let response = GitCommitLookupResponse {
        commit: GitCommitResponse {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            short_hash: "0123456".into(),
            parent_hashes: vec!["89abcdef0123456789abcdef0123456789abcdef".into()],
            author_name: "Lithe Test".into(),
            author_email: "test@example.invalid".into(),
            date: "2026/08/30 12:00".into(),
            date_utc_offset_minutes: Some(480),
            date_unix_seconds: None,
            subject: "Fix commit details".into(),
            decorations: "HEAD -> feature/recent".into(),
        },
        body: "Explain why the change is needed.\n\n- keep blank lines\n  and indentation".into(),
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git commit lookup response should serialize"),
        fixture
    );
}

#[test]
fn commit_lookup_keeps_separator_characters_inside_the_body() {
    // The body may contain the header's unit separator or CRLF line breaks;
    // only the first NUL delimits the header from the free-form message.
    let output = "abc\u{1f}ab\u{1f}p1 p2\u{1f}Dev\u{1f}dev@example.invalid\u{1f}2026/08/30 12:00\u{1f}Subject\u{1f}tag: v1\0Line \u{1f} one\r\n\r\nLine two\n\n";
    let lookup = parse_commit_lookup(output).expect("lookup output should parse");
    assert_eq!(lookup.commit.parent_hashes, ["p1", "p2"]);
    assert_eq!(lookup.commit.subject, "Subject");
    assert_eq!(lookup.commit.decorations, "tag: v1");
    assert_eq!(lookup.body, "Line \u{1f} one\r\n\r\nLine two");

    let subject_only = parse_commit_lookup(
        "abc\u{1f}ab\u{1f}\u{1f}Dev\u{1f}dev@example.invalid\u{1f}2026/08/30 12:00\u{1f}Only\u{1f}\0",
    )
    .expect("subject-only output should parse");
    assert_eq!(subject_only.body, "");
    assert!(parse_commit_lookup("missing delimiter").is_none());
}

#[test]
fn push_preview_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/push-preview-v1.json"
    )))
    .expect("Git push preview fixture should be valid JSON");
    let response = GitPushPreviewResponse {
        local_branch: "feature/core".into(),
        local_head: "2222222222222222222222222222222222222222".into(),
        remote: "origin".into(),
        remote_branch: "feature/core".into(),
        target_kind: GitPushTargetKind::Tracking,
        creates_remote_branch: false,
        remote_tracking_oid: Some("1111111111111111111111111111111111111111".into()),
        upstream: Some("origin/feature/core".into()),
        tags: vec![GitPushTagResponse {
            full_name: "refs/tags/v1.0.0".into(),
            object_id: "3333333333333333333333333333333333333333".into(),
        }],
        commits: vec![GitCommitResponse {
            hash: "2222222222222222222222222222222222222222".into(),
            short_hash: "2222222".into(),
            parent_hashes: vec!["1111111111111111111111111111111111111111".into()],
            author_name: "Lithe Developer".into(),
            author_email: "developer@lithe.local".into(),
            date: "2026/08/31 10:30".into(),
            date_utc_offset_minutes: Some(-300),
            date_unix_seconds: None,
            subject: "Add push preview".into(),
            decorations: "HEAD -> feature/core".into(),
        }],
        has_more: false,
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git push preview should serialize"),
        fixture
    );
}

#[test]
fn command_error_response_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/command-error-response-v1.json"
    )))
    .expect("Git command error response fixture should be valid JSON");
    let response = GitCommandResponse {
        arguments: vec![
            "stash".into(),
            "push".into(),
            "--include-untracked".into(),
            "--message".into(),
            "Lithe Smart Checkout".into(),
        ],
        output: "No local changes to save\n".into(),
        stdout: "No local changes to save\n".into(),
        stderr: String::new(),
        exit_code: 0,
        invocations: vec![GitCommandInvocation {
            arguments: vec![
                "stash".into(),
                "push".into(),
                "--include-untracked".into(),
                "--message".into(),
                "Lithe Smart Checkout".into(),
            ],
            stdout: "No local changes to save\n".into(),
            stderr: String::new(),
            exit_code: 0,
        }],
        operation_error: Some(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git reference",
        )),
        stash_restore: None,
        tag_deletion: None,
        branch_deletion: None,
        history_rewrite: None,
        outcome_authoritative: false,
        warnings: Vec::new(),
    };

    assert_eq!(
        serde_json::to_value(response).expect("Git error response should serialize"),
        fixture
    );
}

#[test]
fn structured_diff_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/git/diff.json"
    )))
    .expect("diff fixture should be valid JSON");
    let patch = fixture["patch"]
        .as_str()
        .expect("fixture patch should be text");
    let (rows, hunks, _) = parse_diff(patch);
    let expected = &fixture["expected"];
    let kinds = rows.iter().map(|row| row.kind.as_str()).collect::<Vec<_>>();
    let old_lines = rows
        .iter()
        .map(|row| row.old_line.map(|line| line as u64))
        .collect::<Vec<_>>();
    let new_lines = rows
        .iter()
        .map(|row| row.new_line.map(|line| line as u64))
        .collect::<Vec<_>>();
    let expected_kinds = expected["rowKinds"]
        .as_array()
        .expect("fixture kinds should be an array")
        .iter()
        .map(|kind| kind.as_str().expect("fixture kind should be text"))
        .collect::<Vec<_>>();
    let expected_old_lines = expected["oldLines"]
        .as_array()
        .expect("fixture old lines should be an array")
        .iter()
        .map(Value::as_u64)
        .collect::<Vec<_>>();
    let expected_new_lines = expected["newLines"]
        .as_array()
        .expect("fixture new lines should be an array")
        .iter()
        .map(Value::as_u64)
        .collect::<Vec<_>>();
    assert_eq!(kinds, expected_kinds);
    assert_eq!(old_lines, expected_old_lines);
    assert_eq!(new_lines, expected_new_lines);
    assert_eq!(
        hunks.len(),
        expected["hunkCount"]
            .as_u64()
            .expect("fixture count should be a number") as usize
    );
}

#[test]
fn pathspec_batches_stay_within_the_budget_and_keep_every_path() {
    let paths = ["aaaa", "bbbb", "cc", "dddddddddd", "e"]
        .map(String::from)
        .to_vec();

    let batches = pathspec_batches(&paths, 8);

    assert_eq!(
        batches,
        vec![&paths[0..2], &paths[2..3], &paths[3..4], &paths[4..5]]
    );
    assert!(pathspec_batches(&[], 8).is_empty());
}
