//! ACP text-shaping helpers.

/// Splits a text field into a `Vec<String>` of non-empty lines, trimming
/// trailing `\r` from each and skipping empties. Used by both the
/// prompt-path user-line construction and the reader-thread parser's
/// text-field collection; the helper is co-located here so neither
/// submodule has to depend on the other for a low-level string utility.
pub fn append_text_lines(text: &str, output: &mut Vec<String>) {
    for line in text.split('\n') {
        let normalized = line.trim_end_matches('\r');
        if !normalized.is_empty() {
            output.push(normalized.to_string());
        }
    }
}
