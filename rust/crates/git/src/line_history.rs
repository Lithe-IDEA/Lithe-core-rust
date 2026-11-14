//! Per-line authorship: `git.blame` and the line history that follows a range
//! through earlier revisions.

use super::*;

/// Largest editor snapshot accepted by `git.lineHistory`; larger documents must
/// use committed state instead of an unsaved buffer.
const LINE_HISTORY_MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

/// Default entry limit for `git.lineHistory` before truncation is reported.
const LINE_HISTORY_DEFAULT_LIMIT: usize = 50;

/// Hard ceiling for a client-requested `git.lineHistory` limit.
const LINE_HISTORY_MAX_LIMIT: usize = 200;

/// Returns line attribution parsed from Git's machine-readable porcelain form.
pub fn blame(request: GitBlameRequest) -> Result<GitBlameResponse, CoreError> {
    if !is_safe_pathspec(&request.path) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git blame contains an invalid path",
        ));
    }
    let response = readonly_command(GitCommandRequest {
        root: request.root,
        arguments: vec![
            "blame".to_string(),
            "--line-porcelain".to_string(),
            "--".to_string(),
            request.path,
        ],
        input: None,
    })?;
    if response.exit_code != 0 {
        return Err(CoreError::new(ErrorCode::ProcessFailed, "Git blame failed")
            .with_details(response.output));
    }

    let mut lines = Vec::new();
    let mut commit_hash = String::new();
    let mut author_name = "Unknown".to_string();
    let mut author_time = 0;
    let mut final_line = 0;
    for line in response.output.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() >= 3 && columns[0].len() == 40 {
            if let Ok(parsed_line) = columns[2].parse::<usize>() {
                commit_hash = columns[0].to_string();
                final_line = parsed_line;
            }
        } else if let Some(value) = line.strip_prefix("author ") {
            author_name = value.to_string();
        } else if let Some(value) = line.strip_prefix("author-time ") {
            author_time = value.parse::<i64>().unwrap_or_default();
        } else if line.starts_with('\t') && final_line > 0 {
            lines.push(GitBlameLineResponse {
                line: final_line,
                commit_hash: commit_hash.clone(),
                author_name: author_name.clone(),
                author_time,
            });
            final_line += 1;
        }
    }
    Ok(GitBlameResponse { lines })
}

/// Normalizes CRLF and bare CR newlines to LF. Line numbers are preserved,
/// so mapping stays valid while both comparison sides share one newline
/// semantics regardless of repository autocrlf settings.
fn normalize_line_history_newlines(content: &str) -> String {
    if content.contains('\r') {
        content.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        content.to_string()
    }
}

/// Counts lines the way editors do: a trailing newline starts one final,
/// empty line that Rust's `lines()` does not yield.
fn line_history_line_count(content: &str) -> usize {
    content.lines().count() + usize::from(content.ends_with('\n'))
}

/// Removes one line-history scratch directory on every exit path.
struct ScopedLineHistoryDirectory(std::path::PathBuf);

impl Drop for ScopedLineHistoryDirectory {
    fn drop(&mut self) {
        fs::cleanup_directory(&self.0);
    }
}

/// Creates an isolated scratch directory under the platform temp root. The
/// snapshot may hold private source text, so POSIX permissions are applied at
/// atomic creation exactly like the Git commit context directories.
fn create_line_history_scratch() -> Result<ScopedLineHistoryDirectory, CoreError> {
    if let Some(directory) =
        fs::remote_temp_directory("lithe-line-history").map_err(line_history_io_error)?
    {
        return Ok(ScopedLineHistoryDirectory(directory));
    }
    let prefix = format!("lithe-line-history-{}", std::process::id());
    let mut attempt = 0u128;
    loop {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
            + attempt;
        let candidate = std::env::temp_dir().join(format!("{prefix}-{stamp}"));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(&candidate) {
            Ok(()) => return Ok(ScopedLineHistoryDirectory(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt += 1;
                continue;
            }
            Err(error) => {
                return Err(CoreError::new(
                    ErrorCode::Unknown,
                    "Could not create a line-history scratch directory",
                )
                .with_details(error.to_string()));
            }
        }
    }
}

pub(crate) fn line_history_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        ErrorCode::Unknown,
        "Could not stage the line-history snapshot",
    )
    .with_details(error.to_string())
}

/// Parses one `@@ -a[,n] +b[,m] @@` header into raw hunk coordinates. A zero
/// count marks an empty side, whose start is the line count before the hunk.
fn parse_line_history_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new, _) = rest.split_once(" @@")?;
    let parse = |text: &str| -> Option<(usize, usize)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = parse(old)?;
    let (new_start, new_count) = parse(new)?;
    Some((old_start, old_count, new_start, new_count))
}

fn parse_line_history_hunks(diff: &str) -> Vec<(usize, usize, usize, usize)> {
    diff.lines()
        .filter_map(parse_line_history_hunk_header)
        .collect()
}

/// Inclusive end of one hunk side; an empty side collapses to its position.
fn line_history_endpoint(start: usize, count: usize) -> usize {
    if count == 0 {
        start
    } else {
        start + count - 1
    }
}

/// Maps snapshot lines onto pinned-revision lines through `-U0` hunks, using
/// the unified-format rule that a zero-count side starts between lines.
/// Returns matched `(snapshot_line, revision_line)` pairs plus unmapped lines
/// flagged as provably inserted (`true`) or unreliably rewritten (`false`).
/// Matched `(snapshot_line, revision_line)` pairs, and the unmapped snapshot
/// lines flagged as inserted (`true`) or rewritten (`false`).
type SnapshotLineMap = (Vec<(usize, usize)>, Vec<(usize, bool)>);

fn map_snapshot_lines(
    snapshot_line_count: usize,
    hunks: &[(usize, usize, usize, usize)],
) -> SnapshotLineMap {
    let mut mapped = Vec::new();
    let mut unmapped = Vec::new();
    let mut snapshot_line = 1usize;
    let mut revision_line = 1usize;
    for &(old_start, old_count, new_start, new_count) in hunks {
        let first_new = if new_count == 0 {
            new_start + 1
        } else {
            new_start
        };
        while snapshot_line < first_new && snapshot_line <= snapshot_line_count {
            mapped.push((snapshot_line, revision_line));
            snapshot_line += 1;
            revision_line += 1;
        }
        if new_count > 0 {
            let replaced_end = new_start + new_count - 1;
            while snapshot_line <= replaced_end && snapshot_line <= snapshot_line_count {
                unmapped.push((snapshot_line, old_count == 0));
                snapshot_line += 1;
            }
        }
        if old_count > 0 {
            // Jump to the first revision line after the replaced block; pure
            // insertions leave the incremental positions aligned already.
            revision_line = old_start + old_count;
        }
    }
    while snapshot_line <= snapshot_line_count {
        mapped.push((snapshot_line, revision_line));
        snapshot_line += 1;
        revision_line += 1;
    }
    (mapped, unmapped)
}

/// Recovers merge relevance from the ordinary first-parent diff, keeping only
/// hunks that actually contain one of the traced line contents. Anchoring by
/// content survives commits that `-L` omitted because they shifted lines or
/// renamed the file after the merge.
/// Translates one set of commit-side line numbers onto the parent side of the
/// same commit through its `-U0` hunks. Lines inside a replacement map onto
/// the parent-side span; lines inserted by the commit disappear; untouched
/// lines shift by the hunk deltas.
fn line_history_translate_lines(
    lines: &std::collections::BTreeSet<usize>,
    hunks: &[(usize, usize, usize, usize)],
) -> std::collections::BTreeSet<usize> {
    let mut translated = std::collections::BTreeSet::new();
    for &line in lines {
        let mut handled = false;
        let mut delta = 0i64;
        for &(old_start, old_count, new_start, new_count) in hunks {
            let new_end = new_start + new_count.saturating_sub(1);
            if new_count > 0 && line >= new_start && line <= new_end {
                if old_count > 0 {
                    // A rewrite maps the new line onto the whole parent span:
                    // when counts differ, the line may descend from any of the
                    // replaced lines (squashing three lines into one keeps the
                    // merge range of the middle line reachable), so relevance
                    // intersection stays sound while remaining a superset.
                    for old_line in old_start..=old_start + old_count - 1 {
                        translated.insert(old_line);
                    }
                }
                handled = true;
                break;
            }
            // A zero-count new side marks the position after the boundary
            // row: that row itself keeps its number, later rows shift.
            let after_hunk = if new_count == 0 {
                line > new_start
            } else {
                line >= new_start + new_count
            };
            if after_hunk {
                delta += old_count as i64 - new_count as i64;
            }
        }
        if !handled {
            let shifted = line as i64 + delta;
            if shifted >= 1 {
                translated.insert(shifted as usize);
            }
        }
    }
    translated
}

/// The ranges a merge touched, the path at the merge, and the path it was
/// renamed from.
type MergeRanges = (Vec<GitLineHistoryRange>, String, Option<String>);

/// Recovers merge relevance by replaying the first-parent chain between the
/// pinned base and the merge's first parent. Text anchoring cannot disambiguate
/// duplicate or blank lines, so every touched commit's diff translates the
/// traced base intervals - and renames translate the path - into the merge's
/// coordinate system before its first-parent diff is filtered.
fn line_history_merge_ranges(
    root: &str,
    base_commit: &str,
    base_intervals: &[(usize, usize)],
    base_path: &str,
    compare_base: &str,
    hash: &str,
) -> Result<Option<MergeRanges>, CoreError> {
    let mut lines: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (start, end) in base_intervals {
        for line in *start..=*end {
            lines.insert(line);
        }
    }
    let mut path = base_path.to_string();
    // Walk backwards from the pinned base towards the merge's first parent,
    // restarting the listing whenever a rename changes the path.
    let mut walk_end = base_commit.to_string();
    loop {
        let listing = readonly_command(GitCommandRequest {
            root: root.to_string(),
            arguments: vec![
                "log".to_string(),
                "--first-parent".to_string(),
                "--format=%H".to_string(),
                // Exclude the merge itself: only commits after it may shift
                // coordinates, which must stay in the merge's own frame.
                format!("{hash}..{walk_end}"),
                "--".to_string(),
                path.clone(),
            ],
            input: None,
        })?;
        if listing.exit_code != 0 {
            return Ok(None);
        }
        let touched: Vec<&str> = listing
            .output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        if touched.is_empty() {
            break;
        }
        let mut renamed_at: Option<String> = None;
        for commit in &touched {
            let parent = readonly_command(GitCommandRequest {
                root: root.to_string(),
                arguments: vec!["rev-parse".to_string(), format!("{commit}^")],
                input: None,
            });
            // Root commits compare against the empty tree, matching git.diff.
            let base_revision = match parent {
                Ok(response) if response.exit_code == 0 => response.output.trim().to_string(),
                _ => empty_tree_oid(root)?.to_string(),
            };
            // The diff runs without a pathspec: restricting it to the walked
            // path would clip renames into synthetic "new file" patches and
            // destroy both the rename pairing and the line translation.
            let diff = readonly_command(GitCommandRequest {
                root: root.to_string(),
                arguments: vec![
                    "diff".to_string(),
                    "--unified=0".to_string(),
                    "--no-ext-diff".to_string(),
                    "--no-textconv".to_string(),
                    "--find-renames".to_string(),
                    base_revision.clone(),
                    commit.to_string(),
                ],
                input: None,
            })?;
            if diff.exit_code != 0 {
                continue;
            }
            let mut hunks: Vec<(usize, usize, usize, usize)> = Vec::new();
            let mut rename_target: Option<String> = None;
            // Collect hunks only from the segment whose new side is the
            // walked path; other files in the commit are irrelevant here.
            let mut segment_matches = false;
            for line in diff.output.lines() {
                if let Some(rest) = line.strip_prefix("diff --git a/") {
                    if let Some((old, new)) = rest.split_once(" b/") {
                        segment_matches = new == path;
                        if segment_matches && old != path {
                            rename_target = Some(old.to_string());
                        }
                    }
                    continue;
                }
                if segment_matches {
                    if let Some(hunk) = parse_line_history_hunk_header(line) {
                        hunks.push(hunk);
                    }
                }
            }
            lines = line_history_translate_lines(&lines, &hunks);
            if let Some(old_name) = rename_target {
                path = old_name;
                // Advance the walk past the rename itself, or a back-and-forth
                // rename (a -> b -> a) would rescan the same commits forever
                // and run the query into its timeout.
                walk_end = base_revision.clone();
                renamed_at = Some(base_revision);
                break;
            }
        }
        if renamed_at.is_none() {
            break;
        }
    }
    if lines.is_empty() {
        return Ok(Some((Vec::new(), path, None)));
    }
    // The supplement diff runs without a pathspec, exactly like the replay:
    // restricting it to the walked path would clip a rename committed by the
    // merge itself into a synthetic whole-file addition and lose the
    // before-content along with precise ranges.
    let diff = readonly_command(GitCommandRequest {
        root: root.to_string(),
        arguments: vec![
            "diff".to_string(),
            "--unified=0".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--find-renames".to_string(),
            compare_base.to_string(),
            hash.to_string(),
        ],
        input: None,
    })?;
    if diff.exit_code != 0 {
        return Ok(None);
    }
    let mut ranges = Vec::new();
    let mut previous_path: Option<String> = None;
    let mut segment_matches = false;
    for line in diff.output.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some((old, new)) = rest.split_once(" b/") {
                segment_matches = new == path;
                if segment_matches && old != path {
                    previous_path = Some(old.to_string());
                }
            }
            continue;
        }
        if !segment_matches {
            continue;
        }
        if let Some((old_start, old_count, new_start, new_count)) =
            parse_line_history_hunk_header(line)
        {
            let new_end = line_history_endpoint(new_start, new_count);
            let intersects = lines
                .iter()
                .any(|line| *line >= new_start && *line <= new_end);
            if intersects {
                ranges.push(GitLineHistoryRange {
                    old_start,
                    old_end: line_history_endpoint(old_start, old_count),
                    new_start,
                    new_end,
                });
            }
        }
    }
    Ok(Some((ranges, path, previous_path)))
}

/// Traces which commits touched one one-based line range of an editor
/// snapshot. The snapshot is mapped onto a pinned HEAD revision through an
/// isolated temp comparison, so unsaved edits stay answerable without ever
/// writing inside the workspace or repository metadata.
pub fn line_history(request: GitLineHistoryRequest) -> Result<GitLineHistoryResponse, CoreError> {
    if !is_safe_pathspec(&request.path) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git line history contains an invalid path",
        ));
    }
    if request.start_line == 0 || request.end_line < request.start_line {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git line history requires an ascending one-based line range",
        ));
    }
    if request.content.len() > LINE_HISTORY_MAX_SNAPSHOT_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git line history snapshot exceeds the size budget",
        )
        .with_details(format!("{} bytes", request.content.len())));
    }
    let limit = request
        .limit
        .unwrap_or(LINE_HISTORY_DEFAULT_LIMIT)
        .clamp(1, LINE_HISTORY_MAX_LIMIT);
    let snapshot = normalize_line_history_newlines(&request.content);
    // Range validation uses the editor's line count, where a trailing newline
    // starts one final empty line; the mapping walk uses the physical line
    // count because that final line has no counterpart in any diff.
    let editor_line_count = line_history_line_count(&snapshot);
    let snapshot_line_count = snapshot.lines().count();
    if request.end_line > editor_line_count.max(1) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git line history range exceeds the snapshot",
        ));
    }

    // Pin one revision so the snapshot mapping and the history walk cannot
    // observe different HEADs when a commit lands mid-query.
    let base_commit = match readonly_command(GitCommandRequest {
        root: request.root.clone(),
        arguments: vec!["rev-parse".to_string(), "HEAD".to_string()],
        input: None,
    }) {
        Ok(response) if response.exit_code == 0 => response.output.trim().to_string(),
        // Only a repository without commits has no line history; every other
        // failure (invalid root, timeout, cancellation) must surface.
        Ok(response) if response.output.contains("unknown revision") => String::new(),
        Ok(response) => {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Git line history could not resolve HEAD",
            )
            .with_details(response.output))
        }
        Err(error) => return Err(error),
    };
    let unmapped_new_lines = || {
        (request.start_line..=request.end_line)
            .map(|line| GitLineHistoryUnmappedLine {
                line,
                kind: "new".to_string(),
            })
            .collect::<Vec<_>>()
    };
    if base_commit.is_empty() {
        // A repository without commits has no line history at all.
        return Ok(GitLineHistoryResponse {
            base_commit,
            unmapped_lines: unmapped_new_lines(),
            commits: Vec::new(),
            truncated: false,
        });
    }

    let head_content = readonly_command(GitCommandRequest {
        root: request.root.clone(),
        arguments: vec![
            "show".to_string(),
            format!("{base_commit}:{}", request.path),
        ],
        input: None,
    });
    let head_content = match head_content {
        Ok(response) if response.exit_code == 0 => response.output,
        // Only a path absent from the pinned revision counts as new; other
        // failures must propagate rather than fabricate "new" lines.
        Ok(response) if response.output.contains("does not exist") => {
            return Ok(GitLineHistoryResponse {
                base_commit,
                unmapped_lines: unmapped_new_lines(),
                commits: Vec::new(),
                truncated: false,
            })
        }
        Ok(response) => {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Git line history could not read the pinned revision",
            )
            .with_details(response.output))
        }
        Err(error) => return Err(error),
    };

    let scratch = create_line_history_scratch()?;
    let head_path = scratch.0.join("head.txt");
    let snapshot_path = scratch.0.join("snapshot.txt");
    let head_content = normalize_line_history_newlines(&head_content);
    fs::write(&head_path, &head_content).map_err(line_history_io_error)?;
    fs::write(&snapshot_path, &snapshot).map_err(line_history_io_error)?;

    // Exit code 1 only reports differences between the two scratch files;
    // anything above that is a genuine failure. Driver overrides keep the
    // hunk coordinates raw and deterministic on every machine.
    let diff = readonly_command(GitCommandRequest {
        root: request.root.clone(),
        arguments: vec![
            "diff".to_string(),
            "--no-index".to_string(),
            "--unified=0".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            head_path.to_string_lossy().to_string(),
            snapshot_path.to_string_lossy().to_string(),
        ],
        input: None,
    })?;
    if diff.exit_code > 1 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git line history mapping failed")
                .with_details(diff.output),
        );
    }
    let hunks = parse_line_history_hunks(&diff.output);
    let (mapped, unmapped) = map_snapshot_lines(snapshot_line_count, &hunks);

    let requested = request.start_line..=request.end_line;
    let mut intervals: Vec<(usize, usize)> = Vec::new();
    for (snapshot_line, revision_line) in mapped {
        if !requested.contains(&snapshot_line) {
            continue;
        }
        match intervals.last_mut() {
            // Split whenever revision lines are not strictly consecutive, so a
            // line deleted between two queried lines never widens the range.
            Some((_, end)) if *end + 1 == revision_line => *end = revision_line,
            _ => intervals.push((revision_line, revision_line)),
        }
    }
    let mut unmapped_lines = unmapped
        .into_iter()
        .filter(|(line, _)| requested.contains(line))
        .map(|(line, inserted)| GitLineHistoryUnmappedLine {
            line,
            kind: if inserted { "new" } else { "unreliable" }.to_string(),
        })
        .collect::<Vec<_>>();
    // The editor's trailing virtual line exists only for display; it never
    // maps onto committed content and is reported as newly added.
    for line in requested.clone().filter(|line| *line > snapshot_line_count) {
        unmapped_lines.push(GitLineHistoryUnmappedLine {
            line,
            kind: "new".to_string(),
        });
    }
    unmapped_lines.sort_unstable_by_key(|entry| entry.line);
    if intervals.is_empty() {
        return Ok(GitLineHistoryResponse {
            base_commit,
            unmapped_lines,
            commits: Vec::new(),
            truncated: false,
        });
    }

    // One process traces every interval; `-L` emits only hunks intersecting
    // the traced lines, and `--first-parent` pins the merge semantics.
    let mut arguments = vec!["log".to_string()];
    for (start, end) in &intervals {
        arguments.push(format!("-L{start},{end}:{}", request.path));
    }
    arguments.push("--first-parent".to_string());
    arguments.push("--patch".to_string());
    // Zero context keeps every emitted hunk exactly the touched lines; default
    // three-line context would report whole-file ranges on small documents.
    arguments.push("--unified=0".to_string());
    arguments.push("--no-color".to_string());
    arguments.push("--no-ext-diff".to_string());
    arguments.push("--no-textconv".to_string());
    arguments.push(format!("--max-count={}", limit + 1));
    arguments.push("--pretty=format:%x1e%H%x1f%an%x1f%at%x1f%P%x1f%s".to_string());
    arguments.push(base_commit.clone());
    let log = readonly_command(GitCommandRequest {
        root: request.root.clone(),
        arguments,
        input: None,
    })?;
    if log.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git line history query failed")
                .with_details(log.output),
        );
    }

    let mut commits: Vec<GitLineHistoryCommit> = Vec::new();
    for record in log.output.split('\x1e') {
        let record = record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let (header, body) = record.split_once('\n').unwrap_or((record, ""));
        let fields: Vec<&str> = header.split('\x1f').collect();
        if fields.len() < 5 {
            continue;
        }
        let hash = fields[0].to_string();
        let parent_hashes = fields[3].split_whitespace().collect::<Vec<_>>();
        let is_merge = parent_hashes.len() > 1;
        let compare_base = parent_hashes
            .first()
            .map(|value| value.to_string())
            .unwrap_or_default();
        // Paths with spaces are not representable in this minimal `diff --git`
        // header parse; traced files are expected to use ordinary names.
        let mut path_at_commit = request.path.clone();
        let mut previous_path = None;
        let mut ranges = Vec::new();
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("diff --git a/") {
                if let Some((old, new)) = rest.split_once(" b/") {
                    path_at_commit = new.to_string();
                    if old != new {
                        previous_path = Some(old.to_string());
                    }
                }
            } else if let Some((old_start, old_count, new_start, new_count)) =
                parse_line_history_hunk_header(line)
            {
                ranges.push(GitLineHistoryRange {
                    old_start,
                    old_end: line_history_endpoint(old_start, old_count),
                    new_start,
                    new_end: line_history_endpoint(new_start, new_count),
                });
            }
        }
        if is_merge && !compare_base.is_empty() {
            // `-L` emits merge headers without hunks, so neither the path nor
            // the ranges come from the log output. Replay the first-parent
            // chain from the pinned base to the merge's parent, translating
            // the base intervals and path through every touched commit's diff;
            // text matching could not disambiguate duplicate or blank lines.
            if let Some((supplement, resolved_path, renamed_from)) = line_history_merge_ranges(
                &request.root,
                &base_commit,
                &intervals,
                &request.path,
                &compare_base,
                &hash,
            )? {
                ranges = supplement;
                if resolved_path != path_at_commit {
                    path_at_commit = resolved_path;
                    previous_path = renamed_from;
                } else if previous_path.is_none() {
                    // The rename happened inside the merge itself; -L already
                    // reported the new path, but the pairing is still news.
                    previous_path = renamed_from;
                }
            }
        }
        commits.push(GitLineHistoryCommit {
            hash,
            kind: if is_merge {
                "merge-first-parent"
            } else {
                "commit"
            }
            .to_string(),
            author_name: fields[1].to_string(),
            author_time: fields[2].parse::<i64>().unwrap_or_default(),
            subject: fields[4].to_string(),
            path_at_commit,
            previous_path,
            compare_base,
            relevant_ranges: ranges,
        });
    }
    let truncated = commits.len() > limit;
    commits.truncate(limit);
    Ok(GitLineHistoryResponse {
        base_commit,
        unmapped_lines,
        commits,
        truncated,
    })
}
