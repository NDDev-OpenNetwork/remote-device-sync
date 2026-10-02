//! Conservative recent goodput from successful, timely media receipts only.
//! Low traffic is not a link-capacity measurement; silence never lowers a rate.
#[derive(Default)]
pub(crate) struct DeliveryRate {
    path: Option<u64>,
    start_ms: u64,
    bytes: u64,
    receipts: u64,
    previous_rate: Option<u64>,
    floor: Option<(u64, u64)>,
}

impl DeliveryRate {
    pub(crate) fn sample(
        &mut self,
        path: Option<u64>,
        now_ms: u64,
        bytes: u64,
        receipts: u64,
        impaired: bool,
    ) -> Option<u64> {
        if path.is_none() || path != self.path || impaired || now_ms < self.start_ms {
            *self = Self {
                path,
                start_ms: now_ms,
                bytes,
                receipts,
                ..Self::default()
            };
            return None;
        }
        let elapsed = now_ms.saturating_sub(self.start_ms);
        if elapsed >= 1000 {
            let delivered = bytes.saturating_sub(self.bytes);
            let count = receipts.saturating_sub(self.receipts);
            let rate = (elapsed <= 2500 && count >= 3 && delivered >= 4096)
                .then(|| delivered.saturating_mul(8000) / elapsed);
            if let Some((previous, current)) = self.previous_rate.zip(rate) {
                // Two adjacent sufficiently populated windows and 20% headroom.
                // This is a floor justified by delivery, never a capacity ceiling.
                let minimum = previous.min(current);
                self.floor = Some((minimum / 5 * 4, now_ms));
            } else {
                self.floor = None;
            }
            self.previous_rate = rate;
            self.start_ms = now_ms;
            self.bytes = bytes;
            self.receipts = receipts;
        }
        self.floor
            .filter(|(_, sampled)| now_ms.saturating_sub(*sampled) <= 2000)
            .map(|(rate, _)| rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_two_windows_and_uses_the_lower_rate_with_headroom() {
        let mut rate = DeliveryRate::default();
        assert_eq!(rate.sample(Some(1), 0, 500, 4, false), None);
        assert_eq!(rate.sample(Some(1), 1000, 250_500, 14, false), None);
        assert_eq!(
            rate.sample(Some(1), 2000, 450_500, 24, false),
            Some(1_280_000)
        );
        assert_eq!(
            rate.sample(Some(1), 2250, 450_500, 24, false),
            Some(1_280_000)
        );
        assert_eq!(
            rate.sample(Some(1), 3000, 450_500, 24, false),
            None,
            "idle traffic is not capacity proof"
        );
    }

    #[test]
    fn failure_path_change_and_silence_revoke_the_observation() {
        for reason in 0..4 {
            let mut rate = DeliveryRate::default();
            rate.sample(Some(1), 0, 0, 0, false);
            rate.sample(Some(1), 1000, 250_000, 10, false);
            assert!(rate.sample(Some(1), 2000, 500_000, 20, false).is_some());
            let (path, time, impaired) = match reason {
                0 => (Some(1), 2250, true),
                1 => (Some(2), 2250, false),
                2 => (None, 2250, false),
                _ => (Some(1), 5000, false),
            };
            assert_eq!(rate.sample(path, time, 500_000, 20, impaired), None);
        }
    }
}
