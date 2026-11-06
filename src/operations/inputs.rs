//! Canonical operation inputs: typed, protocol-neutral parameters.
//!
//! Optionality lives in the type system (`Option`, enums), never in a
//! JSON encoding. No input carries a sender or `on_behalf_of` field:
//! the relay derives attribution and the caller cannot name a
//! different sender. Bundle selection is explicit so qualification
//! happens operation-side (the wire carries no bundle field); the
//! exact defaulting rule follows the step-3 runtime types.

use crate::relay::CredentialDestination;

/// Which bundle namespace a bare target resolves against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BundleScope {
    /// The principal's home (associated) bundle.
    #[default]
    Home,
    /// Every addressable namespace (`*` on the wire surface).
    All,
    /// One concrete namespace.
    Named(String),
}

/// `send` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct SendInput {
    /// Explicit target recipients: bare ids resolve against `bundle`,
    /// `id@namespace` passes through, `id@namespace!relay` routes
    /// cross-relay.
    pub targets: Vec<String>,
    /// Broadcast to all known sessions for the bundle. Mutually
    /// exclusive with a non-empty `targets`.
    pub broadcast: bool,
    /// Message body. Validated non-empty (after trimming); carried
    /// verbatim, untrimmed.
    pub message: String,
    /// Caller correlation id, echoed in the outcome when present.
    pub request_id: Option<String>,
    /// Bundle context bare targets qualify against.
    pub bundle: BundleScope,
}

/// `look` operation input. No timeout: the wire poll's adapter-side
/// prime-wait is not part of the tool contract.
#[derive(Clone, Debug, PartialEq)]
pub struct LookInput {
    /// Bare id (resolves against the principal's home bundle) or
    /// fully-qualified `id@namespace`.
    pub target: String,
    /// Snapshot window size (contract range 1..=1000).
    pub lines: Option<u64>,
    /// Entries to skip from the newest end before the tail-N window.
    pub offset: Option<u64>,
}

/// `list.principals` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct ListPrincipalsInput {
    /// Listing scope; `All` fans out, `Home` uses the associated
    /// bundle, `Named` selects one bundle (or `GLOBAL`).
    pub namespace: BundleScope,
    /// Configured outbound peer alias for foreign discovery. Requires
    /// a concrete `Named` namespace.
    pub relay: Option<String>,
}

/// `list.namespaces` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct ListNamespacesInput {
    /// Configured outbound peer alias; absent lists locally.
    pub relay: Option<String>,
}

/// `list.relays` operation input. Takes no arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListRelaysInput;

/// `list.decisions` operation input. Takes no arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListDecisionsInput;

/// `decide` (MCP `choose`) operation input. The outcome type enforces
/// the contract shape: `selected` requires an option id, `cancelled`
/// forbids one.
#[derive(Clone, Debug, PartialEq)]
pub struct DecideInput {
    /// Pending choice request id from `list.decisions`.
    pub choice_request_id: String,
    /// Decision outcome.
    pub outcome: DecideOutcome,
}

/// Decision outcome with the contract shape in the type.
#[derive(Clone, Debug, PartialEq)]
pub enum DecideOutcome {
    /// Choose an option. Trimmed; validation rejects an
    /// empty-after-trim id.
    Selected { option_id: String },
    /// Decline the request. Carries no option id.
    Cancelled,
}

/// `raww` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct RawwInput {
    /// Bare id, `id@namespace`, or `id@namespace!relay`.
    pub target: String,
    /// Raw text written to the target's input.
    pub text: String,
    /// Suppress the trailing Enter after dispatch.
    pub no_enter: bool,
    /// Caller correlation id, echoed in the outcome when present.
    pub request_id: Option<String>,
}

/// `up` operation input. Operates on the associated bundle; takes no
/// arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpInput;

/// `down` operation input. Operates on the associated bundle; takes no
/// arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownInput;

/// `new.peer` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct NewPeerInput {
    /// Principal id to register, in `<id>@<namespace>` form.
    pub principal_id: String,
    /// Scope recorded on the principal, if any.
    pub scope: Option<String>,
    /// Where the minted credential goes. `Path` carries the caller's
    /// verbatim path (even empty: the relay rejects it rather than
    /// the adapter degrading to `Response`).
    pub destination: CredentialDestination,
}

/// `change.psk` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangePskInput {
    /// Principal id whose credential rotates.
    pub principal_id: String,
    /// Where the rotated credential goes.
    pub destination: CredentialDestination,
}

/// `change.scope` operation input. `scope` is required: an explicit
/// empty string clears the grant; omission is a validation error,
/// never a clear.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeScopeInput {
    /// Peer relay principal id (`<id>@RELAY`).
    pub principal_id: String,
    /// Replacement scope.
    pub scope: String,
}

/// `drop.peer` operation input.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPeerInput {
    /// Principal id to delete.
    pub principal_id: String,
}

/// `link.peer` operation input. The PSK travels in request memory
/// only; it is never logged, persisted, or returned. `Debug` is
/// deliberately redacted so generic debug instrumentation cannot
/// leak the secret.
#[derive(Clone, PartialEq)]
pub struct LinkPeerInput {
    /// Local peer alias whose relay-owned slot receives the credential.
    pub alias: String,
    /// Raw PSK issued by the opposite relay.
    pub psk: String,
}

impl std::fmt::Debug for LinkPeerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkPeerInput")
            .field("alias", &self.alias)
            .field("psk", &"<redacted>")
            .finish()
    }
}
