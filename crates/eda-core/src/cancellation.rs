//! Cooperative cancellation shared by long-running EDA flows.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A cloneable, idempotent cancellation flag.
#[derive(Clone, Debug, Default)]
pub struct FlowCancellation {
    cancelled: Arc<AtomicBool>,
}

impl FlowCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) -> bool {
        self.cancelled.store(true, Ordering::SeqCst);
        true
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_cooperative_and_idempotent() {
        let cancellation = FlowCancellation::new();
        assert!(!cancellation.is_cancelled());
        assert!(cancellation.cancel());
        assert!(cancellation.is_cancelled());
        let clone = cancellation.clone();
        assert!(clone.cancel());
        assert!(cancellation.is_cancelled());
    }
}
