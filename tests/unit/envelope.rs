use agentmux::envelope::{
    AddressIdentity, EnvelopeRenderInput, PromptBatchSettings, TokenizerProfile,
    batch_envelope_groups, parse_address, parse_envelope, parse_tokenizer_profile, render_address,
    render_envelope,
};

fn sample_render_input() -> EnvelopeRenderInput {
    EnvelopeRenderInput {
        message_id: "msg-1".to_string(),
        created_at: "2026-03-05T00:00:00Z".to_string(),
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
        body: "hello from agentmux".to_string(),
    }
}

#[test]
fn envelope_starts_with_leading_boundary_marker() {
    let input = sample_render_input();
    let rendered = render_envelope(&input);
    let first_line = rendered
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("first non-empty line");
    assert_eq!(first_line, "--agentmux-msg1");
}

#[test]
fn envelope_contains_required_headers_and_optional_subject_is_not_required() {
    let rendered = render_envelope(&sample_render_input());
    let lines = rendered.lines().collect::<Vec<_>>();
    assert_eq!(lines.first().copied(), Some("--agentmux-msg1"));
    let header_lines = lines
        .iter()
        .skip(1)
        .copied()
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let required_headers = ["Message-Id:", "Date:", "From:", "To:"];
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
    assert!(!rendered.contains("Envelope-Version:"));
    assert!(!rendered.contains("Content-Type: multipart/mixed;"));
    assert_eq!(
        header_lines
            .iter()
            .filter(|line| line.starts_with("Subject:"))
            .count(),
        0
    );

    let parsed = parse_envelope(&rendered).expect("parse rendered envelope");
    assert_eq!(parsed.subject, None);
    assert_eq!(parsed.message_id, "msg-1");
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
fn envelope_uses_boundary_and_closing_marker_without_top_level_content_type() {
    let rendered = render_envelope(&sample_render_input());
    let lines = rendered.lines().collect::<Vec<_>>();
    let header_end = lines
        .iter()
        .position(|line| line.trim().is_empty())
        .expect("header terminator");
    let boundary_line = lines
        .get(header_end + 1)
        .copied()
        .expect("opening boundary line");
    assert_eq!(boundary_line, "--agentmux-msg1");
    assert!(rendered.trim_end().ends_with("--agentmux-msg1--"));
    assert!(!rendered.contains("Content-Type: multipart/mixed;"));
}

#[test]
fn parser_rejects_missing_boundary_start_marker() {
    let malformed = "\
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>

Content-Type: text/plain; charset=utf-8

hello
";
    let error = parse_envelope(malformed).expect_err("missing boundary should fail");
    assert!(error.to_string().contains("boundary marker"));
}

#[test]
fn parser_rejects_boundary_token_mismatch() {
    let malformed = "\
--agentmux-different
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>

--agentmux-different
Content-Type: text/plain; charset=utf-8

hello
--agentmux-different--
";
    let error = parse_envelope(malformed).expect_err("boundary mismatch should fail");
    assert!(error.to_string().contains("Message-Id-derived boundary"));
}

#[test]
fn parser_rejects_missing_text_plain_body_part() {
    let malformed = "\
--agentmux-msg1
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>

--agentmux-msg1
Content-Type: application/json

{\"hello\":\"world\"}
--agentmux-msg1--
";
    let error = parse_envelope(malformed).expect_err("missing text/plain should fail");
    assert!(error.to_string().contains("text/plain"));
}

#[test]
fn parser_accepts_reserved_path_pointer_part_and_ignores_it_for_body_selection() {
    let envelope = "\
--agentmux-msg1
Message-Id: msg-1
Date: 2026-03-05T00:00:00Z
From: Alpha <session:alpha>
To: Bravo <session:bravo>
Cc: Charlie <session:charlie>

--agentmux-msg1
Content-Type: application/vnd.agentmux.path-pointer+json

{\"label\":\"artifact\",\"local_path\":\"./.auxiliary/temporary/file.txt\"}
--agentmux-msg1
Content-Type: text/plain; charset=utf-8

hello
--agentmux-msg1--
";
    let parsed = parse_envelope(envelope).expect("reserved part envelope should parse");
    assert_eq!(parsed.text_body, "hello");
    assert_eq!(parsed.reserved_path_pointer_parts.len(), 1);
    assert_eq!(parsed.message_id, "msg-1");
    assert_eq!(parsed.from.session_name, "alpha");
    assert_eq!(parsed.to.len(), 1);
    assert_eq!(parsed.to[0].session_name, "bravo");
    assert_eq!(parsed.cc.len(), 1);
    assert_eq!(parsed.cc[0].session_name, "charlie");
}

#[test]
fn batch_envelope_groups_splits_on_budget_and_reports_member_counts() {
    let envelopes = vec![
        "alpha one".to_string(),
        "bravo two".to_string(),
        "charlie three".to_string(),
    ];

    // A budget large enough to hold every envelope yields one group covering all
    // three members, joined in order with a blank line.
    let kept_together = batch_envelope_groups(
        &envelopes,
        PromptBatchSettings {
            max_prompt_tokens: 100,
            tokenizer_profile: TokenizerProfile::WhitespaceRough,
        },
    );
    assert_eq!(kept_together.len(), 1);
    assert_eq!(kept_together[0].member_count, 3);
    assert_eq!(
        kept_together[0].combined_prompt,
        "alpha one\n\nbravo two\n\ncharlie three"
    );

    // A budget too small to combine any pair splits into one single-member group
    // per envelope, preserving order; member counts stay aligned with the source.
    let split = batch_envelope_groups(
        &envelopes,
        PromptBatchSettings {
            max_prompt_tokens: 2,
            tokenizer_profile: TokenizerProfile::WhitespaceRough,
        },
    );
    assert_eq!(split.len(), 3);
    assert!(split.iter().all(|group| group.member_count == 1));
    let prompts: Vec<&str> = split.iter().map(|g| g.combined_prompt.as_str()).collect();
    assert_eq!(prompts, ["alpha one", "bravo two", "charlie three"]);

    // An intermediate budget that fits exactly two envelopes produces a 2-member
    // group followed by a 1-member group — the boundary the ACP task slices on.
    let two_then_one = batch_envelope_groups(
        &envelopes,
        PromptBatchSettings {
            max_prompt_tokens: 4,
            tokenizer_profile: TokenizerProfile::WhitespaceRough,
        },
    );
    assert_eq!(
        two_then_one
            .iter()
            .map(|g| g.member_count)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(two_then_one[0].combined_prompt, "alpha one\n\nbravo two");
    assert_eq!(two_then_one[1].combined_prompt, "charlie three");
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
