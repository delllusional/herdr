use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use wait_timeout::ChildExt;

const STDOUT_LIMIT: usize = 512 * 1024;
const STDERR_LIMIT: usize = 32 * 1024;

#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) timed_out: bool,
}

impl Output {
    pub(crate) fn success(&self) -> bool {
        !self.timed_out && self.status.success()
    }
}

#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

pub(crate) fn run(program: &OsStr, args: &[&OsStr], timeout: Duration) -> io::Result<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| thread::spawn(move || read_capped(stdout, STDOUT_LIMIT)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| thread::spawn(move || read_capped(stderr, STDERR_LIMIT)));

    let status = child.wait_timeout(timeout)?;
    let timed_out = status.is_none();
    if timed_out {
        let _ = child.kill();
    }
    let status = match status {
        Some(status) => status,
        None => child.wait()?,
    };

    let stdout = join_reader(stdout_reader)?;
    // Always drain stderr so a child cannot block on a full pipe. Its content is
    // intentionally not returned because status commands may include private
    // local details in diagnostics.
    let _stderr = join_reader(stderr_reader)?;

    Ok(Output {
        status,
        stdout: stdout.bytes,
        stdout_truncated: stdout.truncated,
        timed_out,
    })
}

fn join_reader(reader: Option<thread::JoinHandle<io::Result<Captured>>>) -> io::Result<Captured> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("command output reader panicked"))?,
        None => Ok(Captured {
            bytes: Vec::new(),
            truncated: false,
        }),
    }
}

fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<Captured> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let keep = remaining.min(read);
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok(Captured { bytes, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_drains_and_marks_truncation() {
        let captured = read_capped(&b"abcdef"[..], 3).expect("capture");
        assert_eq!(captured.bytes, b"abc");
        assert!(captured.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_terminates_child() {
        let output = run(
            OsStr::new("/bin/sh"),
            &[OsStr::new("-c"), OsStr::new("sleep 2")],
            Duration::from_millis(20),
        )
        .expect("run");
        assert!(output.timed_out);
        assert!(!output.success());
    }
}
