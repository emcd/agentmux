use std::{
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use base64::Engine;
use rand::TryRngCore;
use serde_json::json;

use crate::relay::{
    CredentialDestination, RelayError,
    confined::{
        CONFINED_FILE_MODE, ConfineError, ConfinedWrite, confined_stage,
    },
    relay_error,
};

use super::paths::{
    invalid_credential_path, resolve_config_credential_path,
    state_relative_path, validate_output_path,
};
use super::store::PrincipalType;

const PSK_BYTE_LENGTH: usize = 32;

/// Generates a fresh pre-shared key: 32 bytes of OS CSPRNG output encoded as
/// unpadded standard base64.
pub(crate) fn generate_psk() -> String {
    let mut bytes = [0u8; PSK_BYTE_LENGTH];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OsRng must not fail");
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

/// A validated credential destination, resolved to a concrete sink before any
/// principal-store mutation so a rejected destination registers or rotates
/// nothing.
pub(crate) enum StagedCredentialSink {
    /// Return the PSK in the response.
    Response,
    /// Write the PSK to `path`. `create_parents` is set only for relay-owned
    /// `config` paths; caller-named `path` sinks require a pre-existing parent.
    File { path: PathBuf, create_parents: bool },
}

/// A credential written to a temp sibling, awaiting the atomic rename that
/// publishes it. The write lands before the store commit; the rename runs after
/// it, so a store failure discards the staged file and leaves the prior
/// credential (if any) intact.
pub(crate) enum PendingCredentialWrite {
    /// Caller-named `path` sink: temp sibling addressed by pathname. Ancestor
    /// traversal is unchecked (the parents pre-exist outside the trust
    /// anchor); only the final target carries the symlink refusal.
    Direct {
        tmp_path: PathBuf,
        final_path: PathBuf,
    },
    /// Relay-owned sink: staged through confined traversal, published
    /// relative to retained directory handles.
    Confined(ConfinedWrite),
}

impl PendingCredentialWrite {
    /// Atomically publishes the staged credential and returns the written path.
    ///
    /// The temp file's mode is enforced at staging time (before this call), so
    /// the rename is the single, last fallible step — no post-publication work
    /// can report an error after the credential is already in place, except
    /// the confined directory sync, which reports durability uncertainty
    /// without rolling back.
    pub(crate) fn commit(self) -> Result<String, RelayError> {
        match self {
            Self::Direct {
                tmp_path,
                final_path,
            } => {
                fs::rename(&tmp_path, &final_path).map_err(|source| {
                    let _ = fs::remove_file(&tmp_path);
                    relay_error(
                        "internal_credential_write",
                        "failed to finalize credential file",
                        Some(json!({
                            "path": final_path.display().to_string(),
                            "cause": source.to_string(),
                        })),
                    )
                })?;
                Ok(final_path.display().to_string())
            },
            Self::Confined(staged) => {
                let display = staged.display_path().to_string();
                staged.commit().map_err(|error| match error {
                    ConfineError::Symlink { component } => invalid_credential_path(
                        component.as_str(),
                        "credential ancestor is a symlink",
                    ),
                    ConfineError::Exchanged { component } => invalid_credential_path(
                        component.as_str(),
                        "credential ancestor changed during commit",
                    ),
                    ConfineError::DirSync { source } => relay_error(
                        "internal_credential_durability_uncertain",
                        "credential file published but parent-directory sync failed; durability is uncertain",
                        Some(json!({
                            "path": display,
                            "cause": source.to_string(),
                        })),
                    ),
                    ConfineError::Io { source } => relay_error(
                        "internal_credential_write",
                        "failed to finalize credential file",
                        Some(json!({
                            "path": display,
                            "cause": source.to_string(),
                        })),
                    ),
                })
            },
        }
    }

    /// Discards the staged temp file without publishing it.
    pub(crate) fn abort(self) {
        match self {
            Self::Direct { tmp_path, .. } => {
                let _ = fs::remove_file(&tmp_path);
            },
            Self::Confined(staged) => staged.abort(),
        }
    }
}

/// Validates `destination` and resolves it to a concrete sink without writing
/// anything or touching the store. `principal_type` and `principal_id` gate and
/// derive the `config` credential path.
pub(crate) fn stage_credential_sink(
    destination: &CredentialDestination,
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> Result<StagedCredentialSink, RelayError> {
    match destination {
        CredentialDestination::Response => Ok(StagedCredentialSink::Response),
        CredentialDestination::Path { path } => {
            let path = validate_output_path(Path::new(path))?;
            Ok(StagedCredentialSink::File {
                path,
                create_parents: false,
            })
        },
        CredentialDestination::Config => {
            let path = resolve_config_credential_path(
                principal_type,
                principal_id,
                state_root,
            )?;
            Ok(StagedCredentialSink::File {
                path,
                create_parents: true,
            })
        },
    }
}

/// Writes `psk` to the sink's temp sibling (0600, `O_NOFOLLOW`, fsync), creating
/// parent directories only for relay-owned config paths. Returns `None` for the
/// response sink (nothing to publish). Relay-owned sinks stage through
/// confined traversal anchored at `state_root`; caller-named sinks keep the
/// pathname flow with the final-target symlink check only.
pub(crate) fn write_pending_credential(
    sink: &StagedCredentialSink,
    psk: &str,
    state_root: &Path,
) -> Result<Option<PendingCredentialWrite>, RelayError> {
    let StagedCredentialSink::File {
        path,
        create_parents,
    } = sink
    else {
        return Ok(None);
    };
    if *create_parents {
        let relative = state_relative_path(state_root, path);
        let staged =
            confined_stage(state_root, relative, psk.as_bytes(), "cred")
                .map_err(|error| match error {
                    ConfineError::Symlink { component } => {
                        invalid_credential_path(
                            component.as_str(),
                            "credential ancestor is a symlink",
                        )
                    },
                    ConfineError::Exchanged { component } => {
                        invalid_credential_path(
                            component.as_str(),
                            "credential ancestor changed during commit",
                        )
                    },
                    ConfineError::DirSync { .. } => relay_error(
                        "internal_credential_write",
                        "unexpected directory sync during staging",
                        Some(json!({
                            "path": path.display().to_string(),
                        })),
                    ),
                    ConfineError::Io { source } => relay_error(
                        "internal_credential_write",
                        "failed to stage credential file",
                        Some(json!({
                            "path": path.display().to_string(),
                            "cause": source.to_string(),
                        })),
                    ),
                })?;
        return Ok(Some(PendingCredentialWrite::Confined(staged)));
    }
    // `create_new` (O_EXCL) guarantees we materialize a fresh 0600 file rather
    // than truncating a stale sibling whose looser permissions would briefly
    // hold the raw PSK; `O_NOFOLLOW` refuses a symlinked temp.
    let tmp_path = crate::relay::confined::unique_temp_path(path, "cred");
    let mut options = fs::OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .mode(CONFINED_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&tmp_path).map_err(|source| {
        relay_error(
            "internal_credential_write",
            "failed to open credential temp file",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    // Enforce exactly 0600 before the secret is written, then publish via the
    // rename commit point, so no fallible permission step follows publication.
    if let Err(source) = fs::set_permissions(
        &tmp_path,
        fs::Permissions::from_mode(CONFINED_FILE_MODE),
    ) {
        let _ = fs::remove_file(&tmp_path);
        return Err(relay_error(
            "internal_credential_write",
            "failed to set credential temp file mode",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        ));
    }
    let mut file = file;
    let write_result = io::Write::write_all(&mut file, psk.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(source) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(relay_error(
            "internal_credential_write",
            "failed to write credential temp file",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        ));
    }
    Ok(Some(PendingCredentialWrite::Direct {
        tmp_path,
        final_path: path.clone(),
    }))
}
