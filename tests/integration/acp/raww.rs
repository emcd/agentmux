use std::{
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::{ListedSessionTransport, RelayResponse};

use super::helpers::*;

#[test]
fn acp_raww_returns_queued_and_dispatches_prompt() {
    let temporary = GuardedTempDir::new();
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);

    let response = dispatch_raww(
        &config_root,
        &temporary.path().join("tmux.sock"),
        "alpha",
        "bravo",
        "raww-to-acp",
        false,
    );

    let RelayResponse::Raww {
        status,
        target_session,
        transport,
        request_id,
        message_id,
        ..
    } = response
    else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");
    assert_eq!(target_session, "bravo@party");
    assert_eq!(transport, ListedSessionTransport::Acp);
    assert_eq!(request_id.as_deref(), Some("req-acp-raww"));
    assert!(message_id.is_some(), "message_id should be present");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let requests = read_request_log(log_path.as_path());
        if request_count_by_method(requests.as_slice(), "session/prompt") >= 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected ACP session/prompt invocation in log"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
