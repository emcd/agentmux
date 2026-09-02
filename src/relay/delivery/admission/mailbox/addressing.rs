//! Who a mailbox belongs to, in the ledger's own terms.

use crate::protocol::identity::DeliveryTargetId;

use super::super::ledger::AdmissionTargetKey;

pub(super) fn target_key(target: &DeliveryTargetId) -> AdmissionTargetKey {
    AdmissionTargetKey::new(
        target.namespace.as_str(),
        target.runtime_directory.as_path(),
        target.target_session.as_str(),
    )
}
