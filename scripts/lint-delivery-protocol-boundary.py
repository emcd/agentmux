#!/usr/bin/env python3
"""Lint the delivery protocol boundary (`src/protocol/`) for back-edges.

Delivery runs in two directions -- the relay calls a transport to read a target,
and a transport calls the relay to consume a target's mailbox -- and
`src/protocol/` holds the vocabulary both directions name so that neither has to
import the other. A dependency on `crate::relay`, `crate::acp`, `crate::tmux`,
`crate::pty`, or `crate::transports` from inside that boundary means one call
direction has come to need the other's concrete types in order to state its own
contract, which is the coupling the boundary exists to prevent.

Rust cannot express the constraint in the type system while both sides live in
one crate: module visibility governs who may *see* an item, not who may *depend
on* a module. A separate workspace crate would make the rule structural and this
script unnecessary. Until then the rule is mechanical here rather than
aspirational in a doc comment.

**The rule is about path segments, not path roots.** A forbidden module is
rejected wherever it appears in a qualified path, however that path is rooted.
Anchoring to `crate::`/`super::` instead -- the obvious reading -- is defeated by
any rebinding of the crate root:

    use crate as alias;                     // or: extern crate self as alias;
    use alias::{relay::RelayError};

and by every variation on that idea. Rather than enumerate the rebindings, the
check declines to care what a path is rooted at: an import must name the module
it reaches somewhere along the way, so naming is what gets judged. Nothing in
this boundary can legitimately name those five modules at any position in any
path, so the stricter rule costs nothing and removes the whole evasion class at
once.

**Why this is not a grep.** The first version was a line-oriented regex requiring
a forbidden segment immediately after `crate::`. Every one of these creates the
dependency and none matches that shape:

    use crate::{relay::RelayError};
    use crate::{configuration::SessionType, transports::StructuredEntry};
    use crate::{
        relay::RelayError,
        acp::AcpTransport,
    };
    use crate::{transports::{contract::Transport}};

The multi-line form is not merely unmatched but invisible to line-oriented
matching: the forbidden segment sits on a line that does not contain `crate::` at
all, and rustfmt produces exactly that shape for any import list past the width
limit. So this script expands each `use` tree into the full paths it imports --
`crate::{a::B, c::{D, E}}` becomes three paths -- and judges those. Grouped,
nested, aliased, glob and multi-line imports are the same paths once expanded.

Qualified paths outside `use` statements are judged the same way, so a bare
`crate::relay::RelayError` in a signature, or a `relay::RelayError` reached
through a glob import, is caught too. A bare identifier is not a path and is not
judged: a local variable named `relay` is not a dependency on one.

Comments and the contents of string literals are blanked before any of this. A
string cannot create a dependency, and scanning one would reject legitimate
diagnostics, `#[doc = "..."]` content, and this boundary's own documentation of
the rule.

This is a lint rather than a test because what it asserts is a property of the
repository's module graph, not behavior of the crate. As a pre-commit hook it
inherits pre-commit's stashing of unstaged changes, so it judges what is being
committed; run directly, it judges the working tree.

The self-test below runs on every invocation, before the script will report on
the boundary at all. It costs microseconds, and it is what keeps the guard's own
teeth from rotting: a refactor that quietly stops seeing grouped imports, or
alias rebinding, fails here rather than going unnoticed until a back-edge lands.
"""

import re
import sys
from pathlib import Path

BOUNDARY = Path("src/protocol")

# Each name is a sibling of the boundary that must not be reachable from inside
# it, mapped to where it lives. The paths are checked to exist: without that, the
# rule passes vacuously once a module is renamed out from under it -- nothing
# would match because the name is gone, not because the boundary stayed clean.
FORBIDDEN_MODULES = {
    "relay": "src/relay",
    "acp": "src/acp",
    "tmux": "src/tmux",
    "pty": "src/pty",
    "transports": "src/transports",
}

# The leading delimiter is matched but not reported from: a statement following
# `;` on an earlier line would otherwise be attributed to that earlier line. The
# `use` keyword's own position is what a reader needs to find the import.
USE_STATEMENT = re.compile(
    r"(?:^|[;{}\n])\s*(?:pub\s*(?:\([^)]*\)\s*)?)?(use)\s+(.*?);",
    re.DOTALL,
)

# Two or more segments joined by `::`. Requiring the join is what separates a
# path from a bare identifier, so a field or local named `relay` is left alone.
# `r#` is accepted on each segment: a raw identifier names the same module as the
# plain one, so `r#relay` must not read as a different module -- or as the
# module `r`, which is what a matcher unaware of the prefix would see.
#
# A segment is defined by what cannot be in one, not by what can.
#
# Rust identifiers are XID_Start/XID_Continue, which includes combining marks
# and much else that `\w` excludes. Enumerating what an identifier may hold means
# chasing Unicode categories forever, and every character missed breaks the `::`
# chain at that segment -- which stops the matcher seeing the forbidden segment
# joined to it. Inverting the question bounds it: the separators are a short,
# fixed set of ASCII punctuation, and everything else is segment material.
#
# The error direction is deliberate. This class is far wider than Rust accepts,
# so it can only ever match a path that is not one -- a loud, one-line
# correction. A class narrower than Rust accepts silently misses a dependency,
# which is the failure this guard exists to prevent.
SEGMENT_SEPARATORS = r"\s(){}\[\]<>,;:&*+\-/=|!?.'\"#@%^~\\"
SEGMENT = rf"(?:r#)?[^{SEGMENT_SEPARATORS}]+"
QUALIFIED_PATH = re.compile(rf"{SEGMENT}(?:\s*::\s*{SEGMENT})+")

IDENTIFIER_CHARACTER = re.compile(rf"[^{SEGMENT_SEPARATORS}]")

# `use foo as bar;` may separate the token by any whitespace, newlines included,
# and rustfmt produces exactly that for a long grouped import. Splitting on a
# literal " as " leaves the alias attached to the segment, which then matches no
# forbidden module.
ALIAS_SEPARATOR = re.compile(r"\s+as\s+")

# Prefixes a string literal may carry. `b`/`c` select byte and C strings, and a
# trailing `r` makes any of them raw. Missing one means its contents are scanned
# as code, which reports paths that a literal only mentions.
STRING_PREFIXES = frozenset({"", "b", "c", "r", "br", "cr"})


def plain_identifier(segment):
    """Strips a raw-identifier prefix, which names the same item without it."""
    return segment[2:] if segment.startswith("r#") else segment


def blank_span(text):
    """Replaces a span with spaces, keeping newlines so line numbers survive."""
    return "".join("\n" if character == "\n" else " " for character in text)


def match_string_literal(source, index):
    """Returns the end offset of a string literal starting at `index`, or None.

    Handles every prefix in `STRING_PREFIXES` and both raw and escaped forms. A
    prefix is only a prefix when it is not the tail of a longer identifier, so
    the `r` in `my_var"` -- or in `r#relay` -- does not open a literal.
    """
    total = len(source)
    if index > 0 and IDENTIFIER_CHARACTER.match(source[index - 1]):
        return None

    cursor = index
    while cursor < total and cursor - index < 2 and source[cursor] in "brc":
        cursor += 1
    prefix = source[index:cursor]
    if prefix not in STRING_PREFIXES:
        return None

    if prefix.endswith("r"):
        hashes = 0
        while cursor < total and source[cursor] == "#":
            hashes += 1
            cursor += 1
        if cursor >= total or source[cursor] != '"':
            return None
        terminator = '"' + "#" * hashes
        end = source.find(terminator, cursor + 1)
        return total if end == -1 else end + len(terminator)

    if cursor >= total or source[cursor] != '"':
        return None
    cursor += 1
    while cursor < total:
        if source[cursor] == "\\":
            cursor += 2
            continue
        if source[cursor] == '"':
            return cursor + 1
        cursor += 1
    return total


def match_character_literal(source, index):
    """Returns the end offset of a character literal, or None for a lifetime.

    `'a'`, `'\\n'`, `'\\''` and `'\\u{41}'` are literals; `'static` is not. The
    two are told apart by finding the closing quote rather than by lookahead at a
    fixed offset, which is what an escaped quote defeats.
    """
    total = len(source)
    cursor = index + 1
    if cursor >= total:
        return None
    if source[cursor] == "\\":
        cursor += 2
        while cursor < total and source[cursor] != "'":
            cursor += 1
    else:
        cursor += 1
    if cursor < total and source[cursor] == "'":
        return cursor + 1
    return None


def match_block_comment(source, index):
    """Returns the end offset of a block comment, honouring Rust's nesting.

    Rust permits `/* /* */ */`. Stopping at the first `*/` would end the comment
    early and hand the rest of it back as code.
    """
    total = len(source)
    depth = 0
    cursor = index
    while cursor < total:
        if source.startswith("/*", cursor):
            depth += 1
            cursor += 2
            continue
        if source.startswith("*/", cursor):
            depth -= 1
            cursor += 2
            if depth == 0:
                return cursor
            continue
        cursor += 1
    return total


def strip_comments_and_strings(source):
    """Blanks comments and literal contents, preserving offsets and line numbers.

    What survives is exactly the text that can create a dependency. A literal or
    a comment can only ever mention a module, so scanning either produces reports
    that are wrong; blanking real code, by contrast, would lose a back-edge, so
    each construct is recognised by finding its true end rather than by guessing
    at one.
    """
    out = []
    index = 0
    total = len(source)
    while index < total:
        character = source[index]

        if character in 'brc"':
            end = match_string_literal(source, index)
            if end is not None:
                out.append(blank_span(source[index:end]))
                index = end
                continue

        if character == "'":
            end = match_character_literal(source, index)
            if end is not None:
                out.append(blank_span(source[index:end]))
                index = end
                continue

        if source.startswith("//", index):
            end = source.find("\n", index)
            end = total if end == -1 else end
            out.append(blank_span(source[index:end]))
            index = end
            continue

        if source.startswith("/*", index):
            end = match_block_comment(source, index)
            out.append(blank_span(source[index:end]))
            index = end
            continue

        out.append(character)
        index += 1
    return "".join(out)


def split_top_level(text, separator=","):
    """Splits on `separator`, ignoring separators nested inside braces."""
    parts = []
    depth = 0
    current = []
    for character in text:
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
        if character == separator and depth == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(character)
    parts.append("".join(current))
    return parts


def expand_use_tree(tree, prefix=()):
    """Expands one `use` tree into the full paths it imports.

    `crate::{a::B, c::{D, E}}` becomes `crate::a::B`, `crate::c::D`,
    `crate::c::E`. Aliases are dropped, since `use crate::relay as r` imports
    `crate::relay` whatever it is called afterwards.
    """
    paths = []
    for part in split_top_level(tree):
        part = part.strip()
        if not part:
            continue
        brace = part.find("{")
        if brace == -1:
            head = ALIAS_SEPARATOR.split(part, maxsplit=1)[0]
            segments = [s.strip() for s in head.split("::") if s.strip()]
            if segments:
                paths.append(tuple(prefix) + tuple(segments))
            continue
        head = part[:brace].strip().rstrip(":")
        inner = part[brace + 1 :].strip()
        if inner.endswith("}"):
            inner = inner[:-1]
        segments = [s.strip() for s in head.split("::") if s.strip()]
        paths.extend(expand_use_tree(inner, tuple(prefix) + tuple(segments)))
    return paths


def line_of(source, offset):
    return source.count("\n", 0, offset) + 1


def violations_in(source):
    """Returns `(line, name, path)` for every back-edge the source creates.

    One report per forbidden module per line, carrying the longest path seen for
    it, so a path caught by both the import expansion and the qualified-path
    match is reported once.
    """
    code = strip_comments_and_strings(source)
    found = {}

    def record(line, path_segments):
        forbidden = [
            plain_identifier(s)
            for s in path_segments
            if plain_identifier(s) in FORBIDDEN_MODULES
        ]
        if not forbidden:
            return
        rendered = "::".join(path_segments)
        key = (line, forbidden[0])
        if key not in found or len(rendered) > len(found[key]):
            found[key] = rendered

    for match in USE_STATEMENT.finditer(code):
        line = line_of(code, match.start(1))
        for path in expand_use_tree(match.group(2)):
            record(line, path)

    for match in QUALIFIED_PATH.finditer(code):
        segments = [s.strip() for s in match.group(0).split("::")]
        record(line_of(code, match.start()), segments)

    return sorted((line, name, path) for (line, name), path in found.items())


# Each case is `(name, source, should_be_flagged)`. The grouped forms are the
# regression for the first bypass found in review; the alias forms are the
# regression for the second, where a rebinding of the crate root reached a
# forbidden module without ever spelling `crate::` in front of it.
SELFTEST_CASES = [
    ("plain path", "use crate::relay::RelayError;", True),
    ("grouped single", "use crate::{relay::RelayError};", True),
    (
        "grouped mixed",
        "use crate::{configuration::SessionType, transports::StructuredEntry};",
        True,
    ),
    (
        "grouped multi-line",
        "use crate::{\n    relay::RelayError,\n    acp::AcpTransport,\n};",
        True,
    ),
    ("nested group", "use crate::{transports::{contract::Transport}};", True),
    ("bare module in a group", "use crate::{relay};", True),
    ("super chain", "use super::super::transports::TransportError;", True),
    ("aliased module", "use crate::relay as relay_module;", True),
    ("glob", "use crate::tmux::*;", True),
    ("re-export", "pub use crate::{pty::PtyTransport};", True),
    (
        "crate rebound by use",
        "use crate as alias;\nuse alias::{relay::RelayError};",
        True,
    ),
    (
        "crate rebound by extern crate self",
        "extern crate self as alias;\nuse alias::relay::RelayError;",
        True,
    ),
    (
        "module reached through a glob import",
        "use crate::*;\nfn failing() -> relay::RelayError { todo!() }",
        True,
    ),
    # A raw identifier names the same module as the plain one. The grouped form
    # is the one that actually escaped: with no `::` after it, nothing else in
    # the checker gets a second chance at it.
    # An alias may be separated by any whitespace, not the single space a literal
    # split assumes. rustfmt emits the newline form for long grouped imports.
    ("aliased inside a group", "use crate::{relay as r};", True),
    ("aliased across a tab", "use crate::{relay\tas r};", True),
    ("aliased across a newline", "use crate::{relay as\nr};", True),
    # Rust identifiers are Unicode. A forbidden segment joined only to a
    # non-ASCII one is still a dependency on the forbidden module.
    (
        "forbidden module joined to a unicode segment",
        "use crate::*;\nfn failing() -> relay::π { todo!() }",
        True,
    ),
    # Rust identifiers also admit combining marks, which `\w` excludes. This
    # first case was reported even before segments accepted them, because the
    # ASCII-only `relay::RelayError` tail matches on its own -- it pins the
    # behaviour rather than guarding the class.
    (
        "root alias carrying a combining mark",
        "use crate as π́;\n"
        "fn failing() -> π́::relay::RelayError { todo!() }",
        True,
    ),
    # This one is the guard: the forbidden segment is last, so its only join is
    # to the combining-mark identifier, and nothing else can catch it.
    (
        "forbidden module joined only to a combining-mark identifier",
        "use crate as π́;\nfn failing() -> π́::relay { todo!() }",
        True,
    ),
    ("raw identifier", "use crate::r#relay::RelayError;", True),
    ("raw identifier alone in a group", "use crate::{r#relay};", True),
    (
        "raw identifier under a rebound root",
        "use crate as alias;\nuse alias::{r#relay::RelayError};",
        True,
    ),
    (
        "inline path outside a use",
        "fn failing() -> crate::relay::RelayError { todo!() }",
        True,
    ),
    ("standard library", "use std::sync::Arc;", False),
    ("external crate", "use serde::{Deserialize, Serialize};", False),
    ("sibling module", "use super::message::DeliveryEnvelope;", False),
    (
        "permitted crate module",
        "use crate::envelope::{AddressIdentity, render_envelope};",
        False,
    ),
    ("line comment", "// use crate::acp::AcpTransport;", False),
    ("module doc naming the rule", "//! Never import crate::relay here.", False),
    ("block comment", "/* crate::transports::Transport */", False),
    ("intra-doc link", "/// See [`crate::relay::contract`] for the wire.", False),
    # A string cannot create a dependency. Scanning one would reject legitimate
    # diagnostics and documentation, including this boundary's own explanation
    # of the rule it is subject to.
    (
        "forbidden path inside a string literal",
        'const WHY: &str = "does not import crate::relay";',
        False,
    ),
    (
        "forbidden path inside a raw string",
        'const WHY: &str = r#"crate::transports::Transport"#;',
        False,
    ),
    (
        "forbidden path inside a doc attribute",
        '#[doc = "look is answered without crate::relay"]\npub struct Boundary;',
        False,
    ),
    (
        "forbidden path inside a byte string",
        'const B: &[u8] = b"crate::relay";',
        False,
    ),
    (
        "forbidden path inside a raw byte string",
        'const B: &[u8] = br#"crate::relay"#;',
        False,
    ),
    (
        "forbidden path inside a C string",
        'const C: &str = c"crate::relay";',
        False,
    ),
    # Rust block comments nest. Stopping at the first `*/` ends the comment early
    # and hands the rest of it back as code, which then reports what it mentions.
    (
        "forbidden path in a nested block comment",
        "/* outer /* inner */ crate::relay::RelayError */",
        False,
    ),
    # Guards the literal scanner itself: a quote inside a character literal must
    # not be read as opening a string, which would blank the real code after it.
    (
        "back-edge after a quote character literal",
        "const QUOTE: char = '\"';\nuse crate::relay::RelayError;",
        True,
    ),
    (
        "back-edge after an escaped-quote character literal",
        "const QUOTE: char = '\\'';\nuse crate::relay::RelayError;",
        True,
    ),
    (
        "back-edge after a raw string holding a quote",
        'const S: &str = r#"a " b"#;\nuse crate::relay::RelayError;',
        True,
    ),
    (
        "lifetime is not a character literal",
        "pub struct Held<'a> { pub name: &'a str }",
        False,
    ),
    ("bare identifier is not a path", "let relay = 1; let acp = 2;", False),
]


# A violation on the third line, reached by a statement that follows a `;` on an
# earlier line. Attributing it to the delimiter's line instead of the `use`
# keyword's sends a reader to the wrong import.
LINE_ATTRIBUTION_CASE = "use std::sync::Arc;\n\nuse crate::relay::RelayError;"
LINE_ATTRIBUTION_EXPECTED = 3


def run_selftest():
    """Returns a list of failure descriptions; empty when the detector works."""
    failures = []
    for name, source, should_flag in SELFTEST_CASES:
        flagged = bool(violations_in(source))
        if flagged != should_flag:
            expected = "rejected" if should_flag else "accepted"
            failures.append(
                f"self-test case {name!r} should have been {expected}"
            )

    reported = [line for line, _, _ in violations_in(LINE_ATTRIBUTION_CASE)]
    if reported != [LINE_ATTRIBUTION_EXPECTED]:
        failures.append(
            "self-test line attribution should have reported "
            f"[{LINE_ATTRIBUTION_EXPECTED}], reported {reported}"
        )

    return failures


def main():
    failed = False

    for failure in run_selftest():
        print(f"lint-delivery-protocol-boundary: {failure}", file=sys.stderr)
        failed = True
    if failed:
        print(
            "lint-delivery-protocol-boundary: the detector does not behave as "
            "specified, so its verdict on the boundary means nothing",
            file=sys.stderr,
        )
        return 1

    if not BOUNDARY.is_dir():
        print(
            f"lint-delivery-protocol-boundary: {BOUNDARY} is missing; the "
            "back-edge guard covers nothing",
            file=sys.stderr,
        )
        return 1

    sources = sorted(BOUNDARY.rglob("*.rs"))
    if not sources:
        print(
            f"lint-delivery-protocol-boundary: {BOUNDARY} holds no Rust "
            "sources; the back-edge guard covers nothing",
            file=sys.stderr,
        )
        return 1

    for name, location in sorted(FORBIDDEN_MODULES.items()):
        module = Path(location)
        if not module.exists() and not module.with_suffix(".rs").exists():
            print(
                f"lint-delivery-protocol-boundary: forbidden module {name!r} no "
                f"longer exists at {location}; this rule names a module that is "
                "gone, so update it rather than leaving it matching nothing",
                file=sys.stderr,
            )
            failed = True

    for source in sources:
        for line, name, path in violations_in(
            source.read_text(encoding="utf-8")
        ):
            print(
                f"lint-delivery-protocol-boundary: {source}:{line}: "
                f"depends on {name} via {path}",
                file=sys.stderr,
            )
            failed = True

    if failed:
        print(
            "lint-delivery-protocol-boundary: {} must not depend on crate::{{{}}}; "
            "it holds the vocabulary both delivery directions name, so a "
            "dependency on either side makes it one side's module wearing a "
            "neutral name".format(BOUNDARY, ", ".join(sorted(FORBIDDEN_MODULES))),
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
