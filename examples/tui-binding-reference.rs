//! Emits the generated default-binding section of `documentation/usage/tui.md`.
//!
//! `scripts/lint-tui-binding-documentation.sh` runs this and fails when the
//! committed section differs, which is what keeps the guide from drifting;
//! `--fix` on that script splices this output into the guide. Locating the
//! block is the script's job in both directions, so this only ever writes to
//! standard output.
//!
//! This is an example rather than a module of the crate because the crate does
//! not otherwise emit documentation, and because building it from the public
//! surface alone is a claim worth compiling: a caller outside the crate can
//! render its own binding reference from `help_bindings` without reaching into
//! the table. Nothing here decides what a chord is or what it does -- the
//! whole of that is `src/tui/actions/`.

use agentmux::tui::help_bindings;

/// Delimiters of the generated block, emitted here rather than assumed by the
/// lint so the marker text has one definition.
const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED BINDINGS -->";
const END_MARKER: &str = "<!-- END GENERATED BINDINGS -->";

/// Stated inside the block rather than beside it, so an editor who opens the
/// guide and starts typing reads it before the first binding.
///
/// The folding note is here because it explains the shape of what follows --
/// why a reader scanning the list never sees a modified `Enter` -- rather than
/// what the bindings mean. The substance belongs to the guide's own capability
/// section, and is pointed at rather than restated.
const PREAMBLE: &str = "\
<!-- Generated from the binding table in src/tui/actions/bindings.rs.
     Regenerate with: scripts/lint-tui-binding-documentation.sh --fix
     Do not edit between these markers; the pre-commit lint rejects drift. -->

The modified `Enter` forms are folded into the bare one they always match; see
[Terminal keyboard capability](#terminal-keyboard-capability).
";

fn main() {
    print!("{}", binding_reference());
}

fn binding_reference() -> String {
    let mut rendered = String::new();
    rendered.push_str(BEGIN_MARKER);
    rendered.push('\n');
    rendered.push_str(PREAMBLE);
    for section in help_bindings() {
        rendered.push_str(&format!("\n#### {}\n\n", section.heading));
        for entry in section.entries {
            rendered.push_str(&format!(
                "- {} — {}\n",
                chords_of(&entry.chords),
                entry.description
            ));
        }
    }
    rendered.push('\n');
    rendered.push_str(END_MARKER);
    rendered.push('\n');
    rendered
}

/// Renders a presented chord list, marking each chord as code separately.
///
/// The catalogue joins the chords that reach one behavior with a bare slash,
/// which is a separator rather than part of either chord.
fn chords_of(chords: &str) -> String {
    chords
        .split(" / ")
        .map(|chord| format!("`{chord}`"))
        .collect::<Vec<_>>()
        .join(" / ")
}
