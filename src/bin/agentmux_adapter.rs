//! Deterministic reference adapter for the frozen agent-adapter wire.
//!
//! Reads newline-delimited JSON-RPC 2.0 frames on stdin, executes
//! scripted turns from a TOML fixture, and writes response frames on
//! stdout. Diagnostics go to stderr; stdout carries frames only.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::BufWriter;
use std::process::ExitCode;

use agentmux::adapter::handshake::{LimitsProposal, VersionOffer};
use agentmux::adapter::lifecycle::{StderrSink, SystemClock};
use agentmux::adapter::{AdapterConfig, run_stdio};

fn usage() -> ! {
    eprintln!(
        "usage: agentmux-adapter --target <session> --script <path> \
        [--profile generic|acp] [--executable <path>] \
        [--latency <type>=<ms>]... [--max-frame-bytes <n>] \
        [--handshake-timeout-ms <n>] [--liveness-timeout-ms <n>]"
    );
    std::process::exit(2);
}

fn parse_latency(
    declarations: &mut HashMap<String, u64>,
    argument: &str,
) -> bool {
    let Some((content_type, millis)) = argument.split_once('=') else {
        return false;
    };
    let Ok(millis) = millis.parse::<u64>() else {
        return false;
    };
    if millis == 0 {
        return false;
    }
    declarations.insert(content_type.to_string(), millis);
    true
}

fn parse_timeout(value: Option<String>, fallback: u64) -> u64 {
    match value.and_then(|text| text.parse::<u64>().ok()) {
        Some(0) | None => fallback,
        Some(bound) => bound,
    }
}

fn main() -> ExitCode {
    // Ignore SIGPIPE so writes to a closed stdout surface as EPIPE
    // instead of killing the process: the dead-channel rule requires
    // local terminalization, which needs error delivery, not signal
    // death. Portability, not just preference — default dispositions
    // differ across supported Unixes.
    if unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) } == libc::SIG_ERR {
        eprintln!("agentmux-adapter: cannot ignore SIGPIPE");
        return ExitCode::from(2);
    }
    let mut arguments = env::args().skip(1);
    let mut target: Option<String> = None;
    let mut script_path: Option<String> = None;
    let mut profile = "generic".to_string();
    let mut executable = "agentmux-adapter".to_string();
    let mut max_frame_bytes = 1_048_576_u64;
    let mut handshake_timeout_ms: Option<String> = None;
    let mut liveness_timeout_ms: Option<String> = None;
    let mut latency = HashMap::from([("default".to_string(), 5_000_u64)]);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--target" => target = arguments.next(),
            "--script" => script_path = arguments.next(),
            "--profile" => {
                profile = arguments.next().unwrap_or_default();
            },
            "--executable" => {
                executable = arguments.next().unwrap_or_default();
            },
            "--handshake-timeout-ms" => {
                handshake_timeout_ms = arguments.next();
            },
            "--liveness-timeout-ms" => {
                liveness_timeout_ms = arguments.next();
            },
            "--latency" => {
                let Some(value) = arguments.next() else {
                    usage();
                };
                if !parse_latency(&mut latency, &value) {
                    usage();
                }
            },
            "--max-frame-bytes" => {
                let Some(value) = arguments.next() else {
                    usage();
                };
                match value.parse::<u64>() {
                    Ok(bound) if (4096..=67_108_864).contains(&bound) => {
                        max_frame_bytes = bound;
                    },
                    _ => usage(),
                }
            },
            _ => usage(),
        }
    }
    let (Some(target), Some(script_path)) = (target, script_path) else {
        usage()
    };
    let script_text = match fs::read_to_string(&script_path) {
        Ok(text) => text,
        Err(source) => {
            eprintln!("agentmux-adapter: cannot read {script_path}: {source}");
            return ExitCode::from(2);
        },
    };
    let script = match agentmux::adapter::script::parse_script(&script_text) {
        Ok(script) => script,
        Err(error) => {
            eprintln!("agentmux-adapter: invalid script: {error}");
            return ExitCode::from(2);
        },
    };
    let config = AdapterConfig {
        executable,
        profile,
        target_session: target,
        offer: VersionOffer::current(),
        proposal: LimitsProposal {
            max_frame_bytes,
            ..LimitsProposal::negotiation_defaults(latency)
        },
        script,
        handshake_timeout_ms: parse_timeout(handshake_timeout_ms, 30_000),
        liveness_timeout_ms: parse_timeout(liveness_timeout_ms, 60_000),
    };
    // No BufReader and no std::io::stdin() here: both buffer
    // beneath the run loop (BufReader's 8KiB buffer; stdin's shared
    // global input buffer), and prefetched-but-unread bytes escape
    // the RX queue budget. StdinRaw issues one read syscall per call
    // with no userspace buffering, so the run loop's counting reader
    // sees every transport byte exactly once.
    #[cfg(unix)]
    let reader = StdinRaw::new();
    // Non-Unix fallback: the shared stdin handle (buffered; the RX
    // budget does not see its prefetch — Unix deployments, including
    // all conformance harnesses, use the raw reader above).
    #[cfg(not(unix))]
    let reader = std::io::stdin();
    let writer = BufWriter::new(std::io::stdout());
    let mut sink = StderrSink;
    let code = run_stdio(&config, &SystemClock, reader, writer, &mut sink);
    ExitCode::from(code as u8)
}

/// Unbuffered stdin reader: one `read` syscall per call, no
/// userspace buffering anywhere between the pipe and the run loop's
/// byte budget. Retries on interruption; EOF and errors surface
/// unchanged.
#[cfg(unix)]
struct StdinRaw;

#[cfg(unix)]
impl StdinRaw {
    fn new() -> Self {
        Self
    }
}

#[cfg(unix)]
impl std::io::Read for StdinRaw {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let taken = unsafe {
                libc::read(
                    0,
                    buf.as_mut_ptr().cast::<libc::c_void>(),
                    buf.len(),
                )
            };
            if taken < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Ok(taken as usize);
        }
    }
}
