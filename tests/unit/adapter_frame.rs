use std::io::BufReader;

use agentmux::adapter::frame::{
    BOOTSTRAP_MAX_FRAME_BYTES, contract_error, parse_frame, read_frame,
    write_frame,
};
use serde_json::json;

/// A complete frame round-trips byte-identical through the codec.
#[test]
fn frame_round_trip_preserves_bytes() {
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"relay.ping"}"#;
    let mut wire: Vec<u8> = Vec::new();
    write_frame(&mut wire, payload, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("write fits the bootstrap bound");
    let mut reader = BufReader::new(wire.as_slice());
    let back = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("read succeeds")
        .expect("frame present");
    assert_eq!(back, payload);
}

/// A frame that outgrows the bound fails without holding it: the
/// caller observes refusal, not a body-sized allocation.
#[test]
fn oversize_frame_refuses_past_bound() {
    let bound = 16;
    let payload = vec![b'x'; 256];
    let mut wire = payload.clone();
    wire.push(b'\n');
    let mut reader = BufReader::new(wire.as_slice());
    let error = read_frame(&mut reader, bound).expect_err("must refuse");
    assert!(
        error.to_string().contains("16"),
        "unexpected error: {error}"
    );
}

/// A body of exactly the bound reads fine; bound+1 refuses.
#[test]
fn exact_bound_fits_lf() {
    for (body_len, bound, ok) in [(16_usize, 16_usize, true), (17, 16, false)]
    {
        let mut wire = vec![b'x'; body_len];
        wire.push(b'\n');
        let mut reader = BufReader::new(wire.as_slice());
        match read_frame(&mut reader, bound) {
            Ok(Some(frame)) => {
                assert!(ok, "bound {bound} should refuse {body_len} bytes");
                assert_eq!(frame.len(), body_len);
            },
            Ok(None) => panic!("expected a frame"),
            Err(error) => assert!(
                !ok,
                "bound {bound} should accept {body_len} bytes: {error}"
            ),
        }
    }
}

/// CRLF endings strip the carriage return before the bound applies.
#[test]
fn exact_bound_fits_crlf() {
    for (body_len, bound, ok) in [(16_usize, 16_usize, true), (17, 16, false)]
    {
        let mut wire = vec![b'x'; body_len];
        wire.extend_from_slice(b"\r\n");
        let mut reader = BufReader::new(wire.as_slice());
        match read_frame(&mut reader, bound) {
            Ok(Some(frame)) => {
                assert!(ok, "bound {bound} should refuse {body_len} bytes");
                assert_eq!(frame.len(), body_len);
            },
            Ok(None) => panic!("expected a frame"),
            Err(error) => assert!(
                !ok,
                "bound {bound} should accept {body_len} bytes: {error}"
            ),
        }
    }
}

/// Invalid JSON fails as unparseable with the serde reason attached.
#[test]
fn invalid_json_is_unparseable() {
    let error = parse_frame(b"{not json").expect_err("must fail");
    assert!(
        error.to_string().contains("not parseable"),
        "unexpected error: {error}"
    );
}

/// Clean EOF with no pending bytes is not a frame and not an error.
#[test]
fn clean_eof_returns_none() {
    let mut reader = BufReader::new(b"".as_slice());
    let frame = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("clean EOF is not an error");
    assert!(frame.is_none());
}

/// A stream that ends mid-frame is truncated, not cleanly closed.
#[test]
fn truncated_stream_is_unparseable() {
    let mut reader = BufReader::new(b"{\"jsonrpc\"".as_slice());
    let error = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect_err("must fail");
    assert!(
        error.to_string().contains("before the frame newline"),
        "unexpected error: {error}"
    );
}

/// Payloads over the bound fail before any byte reaches the writer.
#[test]
fn write_past_bound_writes_nothing() {
    let mut wire: Vec<u8> = Vec::new();
    let error =
        write_frame(&mut wire, &[b'x'; 32], 16).expect_err("must refuse");
    assert!(wire.is_empty());
    assert!(
        error.to_string().contains("16"),
        "unexpected error: {error}"
    );
}

/// The contract error echoes request id and correlation, and carries
/// the namespaced code in data.
#[test]
fn contract_error_echoes_id_and_correlation() {
    let id = json!("req-7");
    let error = contract_error(
        Some(&id),
        "validation_skew",
        "bad major",
        None,
        Some("c-1"),
    );
    assert_eq!(error["error"]["code"], -32000);
    assert_eq!(error["id"], "req-7");
    let data = &error["error"]["data"];
    assert_eq!(data["code"], "validation_skew");
    assert_eq!(data["correlation_id"], "c-1");
    assert!(data.get("details").is_none());
}

/// correlation_invalid is exempt from the required echo: the builder
/// takes no correlation and the shape still validates.
#[test]
fn correlation_invalid_needs_no_echo() {
    let error =
        contract_error(None, "correlation_invalid", "bad link", None, None);
    assert_eq!(error["error"]["code"], -32000);
    assert!(error["error"]["data"].get("correlation_id").is_none());
}

/// CRLF line endings read as the bare frame.
#[test]
fn crlf_ending_strips_carriage_return() {
    let mut wire = b"{}".to_vec();
    wire.extend_from_slice(b"\r\n");
    let mut reader = BufReader::new(wire.as_slice());
    let back = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("read succeeds")
        .expect("frame present");
    assert_eq!(back, b"{}");
}

/// Back-to-back frames on one stream read as two distinct frames.
#[test]
fn consecutive_frames_read_in_order() {
    let mut wire: Vec<u8> = Vec::new();
    write_frame(&mut wire, b"{\"n\":1}", BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("first write fits");
    write_frame(&mut wire, b"{\"n\":2}", BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("second write fits");
    let mut reader = BufReader::new(wire.as_slice());
    let first = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("first read succeeds")
        .expect("first frame present");
    let second = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("second read succeeds")
        .expect("second frame present");
    assert_eq!(first, b"{\"n\":1}");
    assert_eq!(second, b"{\"n\":2}");
    let end = read_frame(&mut reader, BOOTSTRAP_MAX_FRAME_BYTES)
        .expect("EOF is not an error");
    assert!(end.is_none());
}
