use std::time::Duration;

use lb_chain_service::Slot;
use tokio::sync::mpsc;
use tracing::{error, warn};

use crate::LOG_TARGET;

pub struct FutureProposalDelayer<Proposal> {
    future_slot_margin: Slot,
    slot_duration: Duration,
    sender: mpsc::Sender<Proposal>,
}

impl<Proposal> FutureProposalDelayer<Proposal> {
    pub const fn new(
        future_slot_margin: Slot,
        slot_duration: Duration,
        sender: mpsc::Sender<Proposal>,
    ) -> Self {
        Self {
            future_slot_margin,
            slot_duration,
            sender,
        }
    }
}

impl<Proposal> FutureProposalDelayer<Proposal>
where
    Proposal: Send + 'static,
{
    pub fn try_delay(&self, proposal: Proposal, proposal_slot: Slot, current_slot: Slot) -> bool {
        if proposal_slot > current_slot.saturating_add(self.future_slot_margin) {
            error!(
                target: LOG_TARGET, ?proposal_slot, ?current_slot, margin = ?self.future_slot_margin,
                "Block is from the future beyond margin. Dropping it",
            );
            return false;
        }

        let delay = calculate_delay(proposal_slot, current_slot, self.slot_duration);
        warn!(
            target: LOG_TARGET, ?proposal_slot, ?current_slot, ?delay,
            "Block is from the future but within margin. Delaying it",
        );

        let channel = self.sender.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Err(err) = channel.send(proposal).await {
                error!(target: LOG_TARGET, ?err, "Failed to reschedule delayed proposal");
            }
        });

        true
    }
}

fn calculate_delay(proposal_slot: Slot, current_slot: Slot, slot_duration: Duration) -> Duration {
    slot_duration.saturating_mul(
        u64::from(proposal_slot.saturating_sub(current_slot))
            .try_into()
            .expect("slot diff should fit in u32"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_delay() {
        let slot_duration = Duration::from_secs(1);
        assert_eq!(
            calculate_delay(Slot::from(5), Slot::from(2), slot_duration),
            Duration::from_secs(3),
            "3 slots ahead should give 3x slot duration"
        );
        assert_eq!(
            calculate_delay(Slot::from(5), Slot::from(5), slot_duration),
            Duration::ZERO,
            "same slot should give zero delay"
        );
        assert_eq!(
            calculate_delay(Slot::from(3), Slot::from(5), slot_duration),
            Duration::ZERO,
            "block in the past should give zero delay"
        );
    }

    #[test] // The test returns early before `tokio::spawn`, so no runtime needed.
    fn test_try_delay_drops_when_beyond_margin() {
        let (tx, _rx) = mpsc::channel(1);
        let delayer = FutureProposalDelayer::new(Slot::from(2), Duration::from_secs(1), tx);

        let current_slot = Slot::from(10);
        // 13 > 10 + 2 → beyond margin
        assert!(!delayer.try_delay(42u32, Slot::from(13), current_slot));
    }

    #[tokio::test]
    async fn test_try_delay_accepts_at_margin_boundary() {
        let (tx, mut rx) = mpsc::channel(1);
        let delayer = FutureProposalDelayer::new(Slot::from(2), Duration::from_secs(1), tx);

        let current_slot = Slot::from(10);
        // 12 == 10 + 2: at the boundary, not beyond → accepted
        assert!(delayer.try_delay(123u32, Slot::from(12), current_slot));

        tokio::time::sleep(Duration::from_millis(2500)).await; // +500ms buffer for testing
        assert_eq!(
            rx.try_recv().expect("proposal should be rescheduled"),
            123u32
        );
    }

    #[tokio::test]
    async fn test_try_delay_accepts_when_within_margin() {
        let (tx, mut rx) = mpsc::channel(1);
        let delayer = FutureProposalDelayer::new(Slot::from(2), Duration::from_secs(1), tx);

        let current_slot = Slot::from(10);
        // 11 < 10 + 2: within margin → accepted
        assert!(delayer.try_delay(123u32, Slot::from(11), current_slot));

        tokio::time::sleep(Duration::from_millis(1500)).await; // +500ms buffer for testing
        assert_eq!(
            rx.try_recv().expect("proposal should be rescheduled"),
            123u32
        );
    }

    #[tokio::test]
    async fn test_try_delay_zero_delay_for_current_slot() {
        let (tx, mut rx) = mpsc::channel(1);
        let delayer = FutureProposalDelayer::new(Slot::from(2), Duration::from_secs(1), tx);

        let slot = Slot::from(10);
        // proposal_slot == current_slot → delay = 0
        assert!(delayer.try_delay(123u32, slot, slot));

        tokio::time::sleep(Duration::from_millis(500)).await; // +500ms buffer for testing
        assert_eq!(
            rx.try_recv()
                .expect("proposal should be forwarded immediately"),
            123u32
        );
    }
}
