use agentmux::runtime::inscriptions::{
    append_inscription_record, mcp_inscriptions_path, relay_inscriptions_path,
};
use serde_json::{Value, json};

#[test]
fn resolves_relay_inscriptions_path_at_relay_level() {
    let resolved = relay_inscriptions_path(std::path::Path::new("/inscriptions"));
    assert_eq!(resolved, std::path::Path::new("/inscriptions/relay.log"));
}

#[test]
fn resolves_mcp_inscriptions_path_per_bundle_and_session() {
    let resolved = mcp_inscriptions_path(
        std::path::Path::new("/inscriptions"),
        "party-alpha",
        "session-1",
    );
    assert_eq!(
        resolved,
        std::path::Path::new("/inscriptions/bundles/party-alpha/sessions/session-1/mcp.log")
    );
}

#[test]
fn concurrent_appends_keep_every_record_a_valid_json_line() {
    // Regression for the macOS CI flake where `relay.send.envelope.metadata`
    // inscriptions vanished under concurrent emission: a `writeln!` split each
    // record into a content write and a separate newline write, so concurrent
    // emitters interleaved into non-JSON lines that readers silently dropped.
    // Hammer the append seam from many threads and assert every line still parses
    // and every record survives.
    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("relay.log");
    let threads = 16;
    let records_per_thread = 200;

    let mut handles = Vec::with_capacity(threads);
    for thread_index in 0..threads {
        let path = path.clone();
        handles.push(std::thread::spawn(move || {
            for record_index in 0..records_per_thread {
                append_inscription_record(
                    &path,
                    "relay.test.concurrent",
                    &json!({
                        "thread": thread_index,
                        "record": record_index,
                    }),
                );
            }
        }));
    }
    for handle in handles {
        handle.join().expect("emitter thread joins");
    }

    let contents = std::fs::read_to_string(&path).expect("read inscription log");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(
        lines.len(),
        threads * records_per_thread,
        "every record must occupy its own intact line"
    );
    for line in &lines {
        let parsed: Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("line is not valid JSON: {line}"));
        assert_eq!(
            parsed.get("event").and_then(Value::as_str),
            Some("relay.test.concurrent"),
            "record carries its event field intact: {line}"
        );
    }
}

#[test]
fn inscriptions_paths_sanitize_unsafe_path_segments() {
    let mcp = mcp_inscriptions_path(
        std::path::Path::new("/inscriptions"),
        "party",
        "session/with/slashes",
    );
    assert_eq!(
        mcp,
        std::path::Path::new("/inscriptions/bundles/party/sessions/session_with_slashes/mcp.log")
    );
}
