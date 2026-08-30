//! Who a mailbox belongs to, in the ledger's own terms.

use crate::protocol::identity::ConsumerBinding;

use super::super::ledger::AdmissionTargetKey;

pub(super) fn target_key(binding: &ConsumerBinding) -> AdmissionTargetKey {
    AdmissionTargetKey::new(
        binding.target.namespace.as_str(),
        binding.target.runtime_directory.as_path(),
        binding.target.target_session.as_str(),
    )
}
