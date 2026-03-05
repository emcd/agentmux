use tmuxmux::envelope::{
    AddressIdentity, ENVELOPE_SCHEMA_VERSION, EnvelopeRenderInput, ManifestPreamble,
    PromptBatchSettings, TokenizerProfile, batch_envelopes, parse_address, parse_envelope,
    parse_tokenizer_profile, render_address, render_envelope,
};

fn sample_render_input() -> EnvelopeRenderInput {
    EnvelopeRenderInput {
        manifest: ManifestPreamble {
            schema_version: ENVELOPE_SCHEMA_VERSION.to_string(),
            message_id: "msg-1".to_string(),
            bundle_name: "party".to_string(),
            sender_session: "alpha".to_string(),
            target_sessions: vec!["bravo".to_string()],
            cc_sessions: Some(vec!["charlie".to_string()]),
            created_at: "2026-03-05T00:00:00Z".to_string(),
        },
        from: AddressIdentity {
            session_name: "alpha".to_string(),
            display_name: Some("Alpha".to_string()),
        },
        to: vec![AddressIdentity {
            session_name: "bravo".to_string(),
            display_name: Some("Bravo".to_string()),
        }],
        cc: vec![AddressIdentity {
            session_name: "charlie".to_string(),
            display_name: Some("Charlie".to_string()),
        }],
        subject: None,
        body: "hello from tmuxmux".to_string(),
    }
}

#[test]
fn envelope_starts_with_compact_manifest_line() {
    let input = sample_render_input();
    let rendered = render_envelope(&input);
    let first_line = rendered
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("first non-empty line");
    let expected = serde_json::to_string(&input.manifest).expect("manifest json");
    assert_eq!(first_line, expected);
}

#[test]
fn envelope_contains_required_headers_and_optional_subject_is_not_required() {
    let rendered = render_envelope(&sample_render_input());
    let mut lines = rendered.lines();
    let _manifest = lines.next().expect("manifest line");
    let header_lines = lines
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let required_headers = [
        "Envelope-Version:",
        "Message-Id:",
        "Date:",
        "From:",
        "To:",
        "Content-Type:",
    ];
    for header in required_headers {
        assert_eq!(
            header_lines
                .iter()
                .filter(|line| line.starts_with(header))
                .count(),
            1,
            "required header should appear exactly once: {header}"
        );
    }
    assert_eq!(
        header_lines
            .iter()
            .filter(|line| line.starts_with("Subject:"))
            .count(),
        0
    );

    let parsed = parse_envelope(&rendered).expect("parse rendered envelope");
    assert_eq!(parsed.subject, None);
}

#[test]
fn address_renderer_and_parser_support_display_identity_format() {
    let raw = render_address(&AddressIdentity {
        session_name: "alpha".to_string(),
        display_name: Some("Alpha Operator".to_string()),
    });
    assert_eq!(raw, "Alpha Operator <session:alpha>");
    let parsed = parse_address(&raw).expect("parse rendered address");
    assert_eq!(parsed.session_name, "alpha");
    assert_eq!(parsed.display_name.as_deref(), Some("Alpha Operator"));
}

#[test]
fn envelope_uses_multipart_boundary_and_closing_marker() {
    let rendered = render_envelope(&sample_render_input());
    let content_type_line = rendered
        .lines()
        .find(|line| line.starts_with("Content-Type: multipart/mixed;"))
        .expect("content type header");
    let boundary = content_type_line
        .split("boundary=\"")
        .nth(1)
        .and_then(|value| value.strip_suffix('"'))
        .expect("boundary value");
    assert!(rendered.contains(&format!("--{boundary}\n")));
    assert!(rendered.trim_end().ends_with(&format!("--{boundary}--")));
}

#[test]
fn parser_rejects_missing_manifest_preamble() {
    let malformed = "\
Envelope-Version: 1
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>
Content-Type: multipart/mixed; boundary=\"b\"

--b
Content-Type: text/plain; charset=utf-8

hello
--b--
";
    let error = parse_envelope(malformed).expect_err("missing manifest should fail");
    assert!(error.to_string().contains("manifest"));
}

#[test]
fn parser_rejects_missing_text_plain_body_part() {
    let malformed = "\
{\"schema_version\":\"1\",\"message_id\":\"msg-1\",\"bundle_name\":\"party\",\"sender_session\":\"alpha\",\"target_sessions\":[\"bravo\"],\"created_at\":\"2026-03-05T00:00:00Z\"}
Envelope-Version: 1
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>
Content-Type: multipart/mixed; boundary=\"b\"

--b
Content-Type: application/json

{\"hello\":\"world\"}
--b--
";
    let error = parse_envelope(malformed).expect_err("missing text/plain should fail");
    assert!(error.to_string().contains("text/plain"));
}

#[test]
fn parser_accepts_reserved_path_pointer_part_and_ignores_it_for_body_selection() {
    let envelope = "\
{\"schema_version\":\"1\",\"message_id\":\"msg-1\",\"bundle_name\":\"party\",\"sender_session\":\"alpha\",\"target_sessions\":[\"bravo\"],\"cc_sessions\":[\"charlie\"],\"created_at\":\"2026-03-05T00:00:00Z\"}
Envelope-Version: 1
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>
Cc: Charlie <session:charlie>
Content-Type: multipart/mixed; boundary=\"b\"

--b
Content-Type: application/vnd.tmuxmux.path-pointer+json

{\"label\":\"artifact\",\"local_path\":\"./.auxiliary/temporary/file.txt\"}
--b
Content-Type: text/plain; charset=utf-8

hello
--b--
";
    let parsed = parse_envelope(envelope).expect("reserved part envelope should parse");
    assert_eq!(parsed.text_body, "hello");
    assert_eq!(parsed.reserved_path_pointer_parts.len(), 1);
    assert_eq!(parsed.manifest.target_sessions, vec!["bravo".to_string()]);
    assert_eq!(
        parsed.manifest.cc_sessions,
        Some(vec!["charlie".to_string()])
    );
}

#[test]
fn batching_keeps_order_and_splits_when_budget_would_be_exceeded() {
    let envelopes = vec![
        "alpha one".to_string(),
        "bravo two".to_string(),
        "charlie three".to_string(),
    ];

    let kept_together = batch_envelopes(
        &envelopes,
        PromptBatchSettings {
            max_prompt_tokens: 100,
            tokenizer_profile: TokenizerProfile::WhitespaceRough,
        },
    );
    assert_eq!(kept_together.len(), 1);
    assert!(kept_together[0].contains("alpha one"));
    assert!(kept_together[0].contains("bravo two"));
    assert!(kept_together[0].contains("charlie three"));

    let split = batch_envelopes(
        &envelopes,
        PromptBatchSettings {
            max_prompt_tokens: 2,
            tokenizer_profile: TokenizerProfile::WhitespaceRough,
        },
    );
    assert_eq!(split.len(), 3);
    assert_eq!(split[0], "alpha one");
    assert_eq!(split[1], "bravo two");
    assert_eq!(split[2], "charlie three");
}

#[test]
fn tokenizer_profiles_are_parsed_from_configuration_values() {
    assert_eq!(
        parse_tokenizer_profile("characters_0_point_3"),
        Some(TokenizerProfile::Characters0Point3)
    );
    assert_eq!(
        parse_tokenizer_profile("whitespace"),
        Some(TokenizerProfile::WhitespaceRough)
    );
    assert_eq!(parse_tokenizer_profile("unknown"), None);
}
