//! Parsers for Git's machine-readable reference, commit, name-status and stash output.

use super::*;

pub(crate) fn parse_reference(line: &str) -> Option<GitReferenceResponse> {
    let columns = line.split('\t').collect::<Vec<_>>();
    if columns.len() < 8 || columns[1].ends_with("/HEAD") {
        return None;
    }
    let kind = if columns[0].starts_with("refs/heads/") {
        "local"
    } else if columns[0].starts_with("refs/remotes/") {
        "remote"
    } else {
        "tag"
    };
    let short_name = match kind {
        "local" => columns[0].strip_prefix("refs/heads/"),
        "remote" => columns[0].strip_prefix("refs/remotes/"),
        "tag" => columns[0].strip_prefix("refs/tags/"),
        _ => None,
    }?;
    let upstream_short_name = (!columns[3].is_empty()).then(|| columns[3].to_string());
    let (ahead, behind) = if kind == "local" && upstream_short_name.is_some() {
        parse_tracking_counts(columns.get(5).copied().unwrap_or_default())
    } else {
        (0, 0)
    };
    Some(GitReferenceResponse {
        full_name: columns[0].to_string(),
        // `%(refname:short)` deliberately adds `heads/` or `tags/` when
        // namespaces collide. The typed contract keeps identity in
        // `fullName` and always exposes the namespace-relative short name.
        short_name: short_name.to_string(),
        kind: kind.to_string(),
        peels_to_commit: kind != "tag" || columns[6] == "commit" || columns[7] == "commit",
        is_current: columns[2].trim() == "*",
        upstream_short_name,
        ahead,
        behind,
    })
}

fn parse_tracking_counts(value: &str) -> (usize, usize) {
    let mut ahead = 0;
    let mut behind = 0;
    for component in value.split(',').map(str::trim) {
        if let Some(value) = component.strip_prefix("ahead ") {
            ahead = value.parse().unwrap_or(0);
        } else if let Some(value) = component.strip_prefix("behind ") {
            behind = value.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

pub(crate) fn parse_commit(line: &str) -> Option<GitCommitResponse> {
    let columns = line.split('\u{1f}').collect::<Vec<_>>();
    if columns.len() < 8 {
        return None;
    }
    let (date, date_utc_offset_minutes) = split_commit_date_offset(columns[5]);
    Some(GitCommitResponse {
        hash: columns[0].to_string(),
        short_hash: columns[1].to_string(),
        parent_hashes: columns[2].split_whitespace().map(String::from).collect(),
        author_name: columns[3].to_string(),
        author_email: columns[4].to_string(),
        date: date.to_string(),
        date_utc_offset_minutes,
        date_unix_seconds: history_query::commit_timestamp(date, date_utc_offset_minutes),
        subject: columns[6].to_string(),
        decorations: columns[7].to_string(),
    })
}

/// Splits a `GIT_COMMIT_DATE_ARGUMENT` value into the displayed local date and
/// the author's UTC offset in minutes. Values without a valid `±HHMM` suffix are
/// kept whole with no offset so unexpected Git output stays visible.
fn split_commit_date_offset(value: &str) -> (&str, Option<i32>) {
    let Some((date, zone)) = value.rsplit_once(' ') else {
        return (value, None);
    };
    let bytes = zone.as_bytes();
    if bytes.len() != 5
        || !matches!(bytes[0], b'+' | b'-')
        || !zone[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return (value, None);
    }
    let hours: i32 = zone[1..3].parse().unwrap_or(0);
    let minutes: i32 = zone[3..5].parse().unwrap_or(0);
    if hours > 23 || minutes > 59 {
        return (value, None);
    }
    let sign = if bytes[0] == b'-' { -1 } else { 1 };
    (date, Some(sign * (hours * 60 + minutes)))
}

pub(crate) fn parse_name_status(output: &str) -> Vec<GitFileResponse> {
    output
        .lines()
        .filter_map(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            if columns.len() < 2 || columns.last().is_some_and(|path| path.is_empty()) {
                return None;
            }
            Some(GitFileResponse {
                status: columns[0].to_string(),
                path: columns.last().unwrap_or(&"").to_string(),
                is_untracked: false,
            })
        })
        .collect()
}

pub(crate) fn parse_stash(line: &str) -> Option<GitStashResponse> {
    let columns = line.split('\u{1f}').collect::<Vec<_>>();
    if columns.len() < 3 {
        return None;
    }
    let reference = columns[0].trim();
    if reference.is_empty() {
        return None;
    }
    let subject = columns[1].trim();
    let lower = subject.to_ascii_lowercase();
    let marker = lower.find("on ").or_else(|| lower.find(" on "));
    let branch = marker.and_then(|index| {
        let raw = &subject[index + 3..];
        raw.split_once(':')
            .or_else(|| raw.split_once(','))
            .map(|(branch, _)| branch.trim().to_string())
            .filter(|branch| !branch.is_empty())
    });
    let message = marker
        .and_then(|index| {
            subject[index + 3..]
                .split_once(':')
                .map(|(_, message)| message)
        })
        .or_else(|| subject.split_once(':').map(|(_, message)| message))
        .unwrap_or(subject)
        .trim()
        .to_string();
    Some(GitStashResponse {
        reference: reference.to_string(),
        oid: columns
            .get(3)
            .copied()
            .unwrap_or_default()
            .trim()
            .to_string(),
        message,
        branch,
        date: columns[2].trim().to_string(),
    })
}
