//! Shared delivery-progress diagnostic context.

use serde_json::{Value, json};

use crate::runtime::inscriptions::emit_delivery_diagnostic as emit_diagnostic;

/// Maximum message ids carried by one delivery-progress inscription.
pub const DIAGNOSTIC_MESSAGE_IDS_MAXIMUM: usize = 32;

/// Identity and group correlation shared by delivery-progress diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryDiagnosticContext<'a> {
    pub namespace: &'a str,
    pub target_session: &'a str,
    message_ids: Vec<String>,
    message_ids_total: usize,
}

impl<'a> DeliveryDiagnosticContext<'a> {
    #[must_use]
    pub fn new<I, S>(namespace: &'a str, target_session: &'a str, message_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut ids = Vec::new();
        let mut total = 0;
        for message_id in message_ids {
            if ids.len() < DIAGNOSTIC_MESSAGE_IDS_MAXIMUM {
                ids.push(message_id.as_ref().to_string());
            }
            total += 1;
        }
        Self {
            namespace,
            target_session,
            message_ids: ids,
            message_ids_total: total,
        }
    }

    #[must_use]
    pub fn without_messages(namespace: &'a str, target_session: &'a str) -> Self {
        Self::new(namespace, target_session, std::iter::empty::<&str>())
    }

    #[must_use]
    pub fn message_ids(&self) -> &[String] {
        self.message_ids.as_slice()
    }

    #[must_use]
    pub fn message_ids_total(&self) -> usize {
        self.message_ids_total
    }
}

/// Emits one delivery-progress diagnostic with bounded group correlation.
pub fn emit_delivery_progress(
    event: &str,
    context: &DeliveryDiagnosticContext<'_>,
    mut details: Value,
) {
    let object = details
        .as_object_mut()
        .expect("delivery diagnostic details must be an object");
    object.insert("namespace".to_string(), json!(context.namespace));
    object.insert("target_session".to_string(), json!(context.target_session));
    object.insert("message_ids".to_string(), json!(context.message_ids));
    object.insert(
        "message_ids_total".to_string(),
        json!(context.message_ids_total),
    );
    emit_diagnostic(event, &details);
}
