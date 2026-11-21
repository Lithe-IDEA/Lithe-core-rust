//! `remote.runningPrograms` and `remote.endPrograms`: the processes and
//! terminals a host session still runs (design §9).
//!
//! A client lists them when a window closes its connection, to ask whether
//! they end or keep running, and after reconnecting to a session an earlier
//! connection kept, which it no longer knows the programs of. Ending them
//! is the "End" of that list; keeping them needs nothing. After a
//! reconnection the client may already have started programs of its own,
//! so it can end only the ones it listed.

use crate::sessions::{lookup, request_error};
use lithe_protocol::CoreError;
use lithe_remote::client::{ClientError, RemoteSession};
use lithe_remote::protocol::message::ErrorCode as RemoteErrorCode;
use lithe_remote::protocol::methods::{self, ResourceLabel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// Deadline for one request that lists or ends a program.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a program is a process or a terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramKind {
    Process,
    Terminal,
}

/// One process or terminal the session still runs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningProgram {
    pub kind: ProgramKind,
    /// The id the program was started with; unique per kind.
    pub id: u64,
    /// Program and arguments, user content the client shows unchanged.
    pub argv: Vec<String>,
    /// The label the program was started with, if any.
    pub label: Option<ResourceLabel>,
    /// Milliseconds the program has run, on the host's clock.
    pub running_ms: u64,
}

/// The processes and terminals `session` still runs, processes first.
pub(crate) fn running(session: &RemoteSession) -> Result<Vec<RunningProgram>, ClientError> {
    let state = session.state()?;
    let processes = state.processes.into_iter().map(|process| RunningProgram {
        kind: ProgramKind::Process,
        id: process.process_id,
        argv: process.argv,
        label: process.label,
        running_ms: process.running_ms,
    });
    let terminals = state.terminals.into_iter().map(|terminal| RunningProgram {
        kind: ProgramKind::Terminal,
        id: terminal.terminal_id,
        argv: terminal.argv,
        label: terminal.label,
        running_ms: terminal.running_ms,
    });
    Ok(processes.chain(terminals).collect())
}

/// `remote.runningPrograms`: the session.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramsRequest {
    session_id: String,
}

/// `remote.endPrograms`: the session, and the programs to end, as
/// `remote.runningPrograms` listed them; none ends every program.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndRequest {
    session_id: String,
    #[serde(default)]
    programs: Option<Vec<ProgramRef>>,
}

/// A program of `remote.endPrograms`: its kind and id.
#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramRef {
    kind: ProgramKind,
    id: u64,
}

/// Lists the processes and terminals the session still runs.
pub fn list(request: ProgramsRequest) -> Result<Value, CoreError> {
    let connection = lookup(&request.session_id)?;
    let programs =
        running(&connection.session).map_err(|error| request_error(&request.session_id, error))?;
    Ok(json!({ "programs": programs }))
}

/// Ends the processes and terminals the session still runs, every one or
/// those `programs` names: processes get `SIGTERM` for their whole process
/// session, terminals hang up. Returns how many were signalled; a program
/// that ended in the meantime, or that `programs` names but the session does
/// not run, is not counted. A program that ignores the signal is still
/// listed afterwards.
pub fn end(request: EndRequest) -> Result<Value, CoreError> {
    let connection = lookup(&request.session_id)?;
    let session = &connection.session;
    let failed = |error| request_error(&request.session_id, error);
    let chosen = |program: &RunningProgram| {
        request.programs.as_ref().is_none_or(|programs| {
            programs.contains(&ProgramRef {
                kind: program.kind,
                id: program.id,
            })
        })
    };
    let mut ended = 0_usize;
    for program in running(session).map_err(failed)?.into_iter().filter(chosen) {
        let (method, params) = match program.kind {
            ProgramKind::Process => (
                methods::PROCESS_KILL,
                json!({ "processId": program.id, "force": false }),
            ),
            ProgramKind::Terminal => (methods::TERMINAL_CLOSE, json!({ "terminalId": program.id })),
        };
        match session.request_value(method, params, REQUEST_TIMEOUT) {
            Ok(_) => ended += 1,
            // log-exempt: The program ended between listing and signalling, which is what the caller asked for.
            Err(ClientError::Remote(error)) if error.code == RemoteErrorCode::UnknownResource => {}
            Err(error) => return Err(failed(error)),
        }
    }
    Ok(json!({ "ended": ended }))
}
