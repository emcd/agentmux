//! Deterministic reference adapter for the frozen agent-adapter wire.
//!
//! Reads newline-delimited JSON-RPC 2.0 frames on stdin, executes
//! scripted turns from a TOML fixture, and writes response frames on
//! stdout. Diagnostics go to stderr; stdout carries frames only.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;

use agentmux::adapter::handshake::{LimitsProposal, VersionOffer};
use agentmux::adapter::lifecycle::{StderrSink, SystemClock};
use agentmux::adapter::{AdapterConfig, run_stdio};

fn usage() -> ! {
    eprintln!(
        "usage: agentmux-adapter --target <session> --script <path> \
        [--profile generic|acp] [--executable <path>] \
        [--latency <type>=<ms>]... [--max-frame-bytes <n>]"
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

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let mut target: Option<String> = None;
    let mut script_path: Option<String> = None;
    let mut profile = "generic".to_string();
    let mut executable = "agentmux-adapter".to_string();
    let mut max_frame_bytes = 1_048_576_u64;
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
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut sink = StderrSink;
    let code =
        run_stdio(&config, &SystemClock, &mut reader, &mut writer, &mut sink);
    let _ = writer.flush();
    ExitCode::from(code as u8)
}
