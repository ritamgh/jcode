use crate::message::Message;
use std::sync::atomic::{AtomicU64, Ordering};

static TOTAL_SAVED_TOKENS: AtomicU64 = AtomicU64::new(0);
static LATEST_SAVED_TOKENS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct InterlangSnapshot {
    pub total_saved_tokens: u64,
    pub latest_saved_tokens: u64,
}

pub fn snapshot() -> Option<InterlangSnapshot> {
    let total_saved_tokens = TOTAL_SAVED_TOKENS.load(Ordering::Relaxed);
    let latest_saved_tokens = LATEST_SAVED_TOKENS.load(Ordering::Relaxed);
    if total_saved_tokens == 0 && latest_saved_tokens == 0 {
        None
    } else {
        Some(InterlangSnapshot {
            total_saved_tokens,
            latest_saved_tokens,
        })
    }
}

pub fn compress_messages_for_request(messages: &[Message]) -> Vec<Message> {
    // Keep the patch hook in the provider request path. The 0.12 port keeps
    // behavior lossless by default because the external llm-interlang compressor
    // is not available in-tree.
    LATEST_SAVED_TOKENS.store(0, Ordering::Relaxed);
    messages.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_hook_is_lossless_without_external_compressor() {
        let messages: Vec<Message> = Vec::new();
        let compressed = compress_messages_for_request(&messages);
        assert_eq!(compressed.len(), messages.len());
    }
}
