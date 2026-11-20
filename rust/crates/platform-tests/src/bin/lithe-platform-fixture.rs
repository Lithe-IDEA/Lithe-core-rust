//! The program that `lithe-platform-tests` starts in place of a real tool.
//!
//! The first argument selects a [`Mode`]; that type describes each one. Every
//! mode exits after [`LIFETIME_LIMIT`] at the latest, so a test that fails to
//! end a fixture does not leave it running.

use lithe_platform_tests::{
    Mode, ECHO_FILE_ENV, FAILURE_ERROR, FAILURE_EXIT_CODE, FAILURE_OUTPUT, LIFETIME_EXIT_CODE,
    LIFETIME_LIMIT, READY_PREFIX, STREAM_DURATION, STREAM_LINE,
};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command, ExitCode};
use std::time::Instant;

fn main() -> ExitCode {
    std::thread::spawn(|| {
        std::thread::sleep(LIFETIME_LIMIT);
        std::process::exit(LIFETIME_EXIT_CODE);
    });
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments
        .next()
        .and_then(|name| name.into_string().ok())
        .and_then(|name| Mode::from_name(&name));
    let Some(mode) = mode else {
        eprintln!("usage: lithe-platform-fixture <mode> [argument...]");
        return ExitCode::from(2);
    };
    let result = match mode {
        Mode::Hold => hold(),
        Mode::Parent => parent(),
        Mode::Failure => fail(),
        Mode::Stream => {
            stream();
            Ok(())
        }
        Mode::EchoArguments => echo(arguments),
        Mode::AskPass => askpass(arguments),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lithe-platform-fixture {}: {error}", mode.name());
            ExitCode::FAILURE
        }
    }
}

fn hold() -> io::Result<()> {
    let mut output = io::stdout().lock();
    writeln!(output, "{READY_PREFIX}{}", std::process::id())?;
    output.flush()?;
    drop(output);
    io::copy(&mut io::stdin().lock(), &mut io::sink())?;
    Ok(())
}

fn parent() -> io::Result<()> {
    // The child inherits every standard stream, as a runtime started by a
    // launcher script does.
    let status = Command::new(std::env::current_exe()?)
        .arg(Mode::Hold.name())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("the child ended with {status}")))
    }
}

fn fail() -> ! {
    println!("{FAILURE_OUTPUT}");
    eprintln!("{FAILURE_ERROR}");
    std::process::exit(FAILURE_EXIT_CODE);
}

fn stream() {
    let deadline = Instant::now() + STREAM_DURATION;
    let mut output = io::stdout().lock();
    while Instant::now() < deadline {
        if writeln!(output, "{STREAM_LINE}").is_err() {
            return;
        }
    }
}

fn echo(arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    let file = std::env::var_os(ECHO_FILE_ENV)
        .ok_or_else(|| io::Error::other(format!("{ECHO_FILE_ENV} is not set")))?;
    let lines: Vec<_> = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    std::fs::write(file, lines.join("\n"))
}

fn askpass(mut arguments: impl Iterator<Item = OsString>) -> ! {
    let prompt = arguments
        .next()
        .map(|prompt| prompt.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::process::exit(lithe_git_host::authentication::helper_main(&prompt));
}
