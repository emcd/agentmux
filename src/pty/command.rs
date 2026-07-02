//! Pty transport command parsing.
//!
//! The per-coder `[coders.<id>.pty].initial-command` (and
//! `resume-command`) is a TOML string the bootstrap path renders
//! against the per-session template (e.g. `codex resume
//! {coder-session-id}`). The rendered string is a shell-style command
//! line with program + args + shell quoting, not a single executable
//! path. [`portable_pty::CommandBuilder::new`] takes a program path;
//! passing the whole rendered string would try to exec a literal
//! binary named `"codex resume abc-123"` and fail.
//!
//! [`tokenize_command`] splits the rendered string into argv tokens
//! using shell-style quoting rules, returning the program as the first
//! token and the args as the rest. Callers (e.g. [`PtyTransport::startup`])
//! pass the first token to [`CommandBuilder::new`] and the remainder
//! via [`CommandBuilder::arg`].
//!
//! [`PtyTransport::startup`]: crate::pty::PtyTransport::startup

use std::{fmt, result::Result as StdResult};

/// Errors that can occur while tokenizing a Pty command string.
#[derive(Debug, PartialEq, Eq)]
pub enum CommandParseError {
    /// The string contains an unterminated quoted region (e.g. an
    /// opening `"` with no closing match).
    UnterminatedQuote(String),
    /// The string tokenized to an empty argv (only whitespace and/or
    /// empty quoted strings).
    Empty(String),
}

impl fmt::Display for CommandParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedQuote(s) => {
                write!(f, "unterminated quoted region in command string: {s:?}")
            }
            Self::Empty(s) => write!(f, "command string tokenized to empty argv: {s:?}"),
        }
    }
}

impl std::error::Error for CommandParseError {}

/// Tokenize a Pty command string into argv tokens using shell-style
/// quoting rules (single quotes, double quotes, backslash escapes).
/// Returns the program as `tokens[0]` and the args as the rest.
///
/// Examples:
///
/// ```text
/// "codex resume abc-123"     -> ["codex", "resume", "abc-123"]
/// "sh -lc 'exec sleep 45'"   -> ["sh", "-lc", "exec sleep 45"]
/// "/usr/local/bin/my-tool"   -> ["/usr/local/bin/my-tool"]
/// ```
pub fn tokenize_command(input: &str) -> Result<Vec<String>, CommandParseError> {
    let tokens = shell_words::split(input)
        .map_err(|e| CommandParseError::UnterminatedQuote(format!("{e}: {input:?}")))?;
    if tokens.is_empty() {
        return Err(CommandParseError::Empty(input.to_string()));
    }
    Ok(tokens)
}

/// Convenience: split argv tokens into `(program, args)`. The first
/// token is the program; the remainder are args. Returns
/// `Err(CommandParseError::Empty)` if the input is empty.
pub fn program_and_args(input: &str) -> StdResult<(String, Vec<String>), CommandParseError> {
    let tokens = tokenize_command(input)?;
    let mut iter = tokens.into_iter();
    let program = iter.next().expect("tokenize_command returned non-empty");
    let args = iter.collect();
    Ok((program, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_program() {
        assert_eq!(
            tokenize_command("/bin/bash").unwrap(),
            vec!["/bin/bash".to_string()],
        );
    }

    #[test]
    fn tokenize_program_with_args() {
        assert_eq!(
            tokenize_command("codex resume abc-123").unwrap(),
            vec![
                "codex".to_string(),
                "resume".to_string(),
                "abc-123".to_string(),
            ],
        );
    }

    #[test]
    fn tokenize_shell_invocation_with_quoted_arg() {
        assert_eq!(
            tokenize_command("sh -lc 'exec sleep 45'").unwrap(),
            vec![
                "sh".to_string(),
                "-lc".to_string(),
                "exec sleep 45".to_string(),
            ],
        );
    }

    #[test]
    fn tokenize_double_quoted_arg() {
        assert_eq!(
            tokenize_command(r#"echo "hello world""#).unwrap(),
            vec!["echo".to_string(), "hello world".to_string()],
        );
    }

    #[test]
    fn tokenize_escaped_space() {
        assert_eq!(
            tokenize_command(r"my\ tool --flag").unwrap(),
            vec!["my tool".to_string(), "--flag".to_string()],
        );
    }

    #[test]
    fn tokenize_unterminated_quote_errors() {
        let err = tokenize_command(r#"sh -c "echo hi"#).unwrap_err();
        assert!(matches!(err, CommandParseError::UnterminatedQuote(_)));
    }

    #[test]
    fn tokenize_empty_string_errors() {
        let err = tokenize_command("").unwrap_err();
        assert!(matches!(err, CommandParseError::Empty(_)));
    }

    #[test]
    fn tokenize_whitespace_only_errors() {
        let err = tokenize_command("   \t  ").unwrap_err();
        assert!(matches!(err, CommandParseError::Empty(_)));
    }

    #[test]
    fn program_and_args_splits_first_token() {
        let (program, args) = program_and_args("codex resume abc-123").unwrap();
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["resume".to_string(), "abc-123".to_string()]);
    }

    #[test]
    fn program_and_args_no_args() {
        let (program, args) = program_and_args("/bin/bash").unwrap();
        assert_eq!(program, "/bin/bash");
        assert!(args.is_empty());
    }
}
