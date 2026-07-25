mod bridge;
mod command;
mod dictation;
mod dontspeak;
mod herdr;
mod model;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args();
    let _program = args.next();
    let result = match (args.next().as_deref(), args.next()) {
        (Some("startup"), None) => bridge::start_detached(),
        (Some("bridge"), None) => bridge::run(),
        (Some("open"), None) => bridge::open_current_dictation(),
        (Some("dictation"), None) => dictation::run(),
        _ => {
            eprintln!("usage: dontspeak-voice <startup|bridge|open|dictation>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dontspeak-voice: {error}");
            ExitCode::FAILURE
        }
    }
}
