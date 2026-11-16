//! Shared policy for choosing when subscriptions are due for automatic refresh.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use vpn_domain::client::Subscription;

#[derive(Default)]
pub struct AutomaticRefresh {
    launch_started: bool,
    launch_pending: HashSet<String>,
    last_attempt: HashMap<String, Attempt>,
}

struct Attempt {
    at: Instant,
    unix: u64,
    failed: bool,
}

impl AutomaticRefresh {
    pub fn start_launch(&mut self, subscriptions: &[Subscription]) {
        if self.launch_started {
            return;
        }
        self.launch_started = true;
        self.launch_pending = subscriptions
            .iter()
            .filter(|item| item.provider.refresh_on_launch())
            .map(|item| item.subscription_uuid.clone())
            .collect();
    }

    pub fn next<'a>(
        &self,
        subscriptions: &'a [Subscription],
        active: Option<&str>,
        now: Instant,
        unix: u64,
        include_launch: bool,
    ) -> Option<(&'a str, Duration)> {
        subscriptions
            .iter()
            .filter(|item| active != Some(item.subscription_uuid.as_str()))
            .filter_map(|item| {
                Some((
                    item.subscription_uuid.as_str(),
                    self.delay(item, unix, now, include_launch)?,
                ))
            })
            .min_by_key(|(_, after)| *after)
    }

    fn delay(
        &self,
        item: &Subscription,
        unix: u64,
        now: Instant,
        include_launch: bool,
    ) -> Option<Duration> {
        let interval = interval(item);
        let attempt = self.last_attempt.get(&item.subscription_uuid);
        let retry = interval.map_or(Duration::ZERO, |interval| {
            retry_delay(item, attempt, unix, now, interval)
        });
        if include_launch
            && item.provider.refresh_on_launch()
            && self.launch_pending.contains(&item.subscription_uuid)
        {
            return Some(retry);
        }
        let interval = interval?;
        let since_success = item.provider.last_success_unix.map_or(interval, |last| {
            Duration::from_secs(unix.saturating_sub(last))
        });
        let since_attempt = attempt.map_or(interval, |last| now.saturating_duration_since(last.at));
        Some(
            interval
                .saturating_sub(since_success.min(since_attempt))
                .max(retry),
        )
    }

    /// Include subscriptions skipped during an active connection in the next platform wakeup.
    pub fn periodic_deadline(&self, subscriptions: &[Subscription]) -> Option<u64> {
        subscriptions
            .iter()
            .filter_map(|item| {
                let interval = interval(item)?.as_secs();
                let attempted = self
                    .last_attempt
                    .get(&item.subscription_uuid)
                    .map(|attempt| attempt.unix);
                let last = item
                    .provider
                    .last_success_unix
                    .max(attempted)
                    .max(item.provider.last_automatic_failure_unix);
                Some(last.map_or(1, |unix| unix.saturating_add(interval)))
            })
            .min()
    }

    pub fn attempted(&mut self, uuid: &str, now: Instant, unix: u64, failed: bool) {
        self.launch_pending.remove(uuid);
        self.last_attempt.insert(
            uuid.to_owned(),
            Attempt {
                at: now,
                unix,
                failed,
            },
        );
    }

    pub fn forget(&mut self, uuid: &str) {
        self.launch_pending.remove(uuid);
        self.last_attempt.remove(uuid);
    }
}

fn retry_delay(
    item: &Subscription,
    attempt: Option<&Attempt>,
    unix: u64,
    now: Instant,
    interval: Duration,
) -> Duration {
    let persisted = item
        .provider
        .last_automatic_failure_unix
        .map_or(interval, |last| {
            Duration::from_secs(unix.saturating_sub(last))
        });
    let local = attempt
        .filter(|value| value.failed)
        .map_or(interval, |value| now.saturating_duration_since(value.at));
    interval.saturating_sub(persisted.min(local))
}

fn interval(item: &Subscription) -> Option<Duration> {
    Some(Duration::from_secs(
        u64::from(item.provider.refresh_interval_hours()?) * 3600,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_domain::client::Secret;
    use vpn_domain::provider::{ProviderKey, ProviderState};

    fn periodic_subscription() -> Subscription {
        let mut provider = ProviderState {
            last_success_unix: Some(1_000),
            ..ProviderState::default()
        };
        provider
            .metadata
            .values
            .insert(ProviderKey::ProfileUpdateInterval, Secret::new("1"));
        Subscription {
            auto_title: false,
            subscription_uuid: "fixture".into(),
            title: "Fixture".into(),
            url: Secret::new("https://example.invalid"),
            provider,
            unavailable_profiles: Vec::new(),
        }
    }

    #[test]
    fn wall_clock_jump_does_not_skip_retry_delay() {
        let subscriptions = [periodic_subscription()];
        let now = Instant::now();
        let mut schedule = AutomaticRefresh::default();
        assert_eq!(
            schedule.next(&subscriptions, None, now, 5_000, false),
            Some(("fixture", Duration::ZERO))
        );
        schedule.attempted("fixture", now, 5_000, true);
        assert_eq!(schedule.periodic_deadline(&subscriptions), Some(8_600));
        assert_eq!(
            schedule.next(
                &subscriptions,
                None,
                now + Duration::from_secs(3_599),
                9_000,
                false,
            ),
            Some(("fixture", Duration::from_secs(1)))
        );
    }

    #[test]
    fn active_subscription_is_skipped_until_disconnect() {
        let subscriptions = [periodic_subscription()];
        let now = Instant::now();
        let schedule = AutomaticRefresh::default();
        assert_eq!(
            schedule.next(&subscriptions, Some("fixture"), now, 5_000, true),
            None
        );
        assert_eq!(schedule.periodic_deadline(&subscriptions), Some(4_600));
        assert_eq!(
            schedule.next(&subscriptions, None, now, 5_000, true),
            Some(("fixture", Duration::ZERO))
        );
    }

    #[test]
    fn saved_failure_defers_periodic_and_launch_refresh_after_restart() {
        let mut subscription = periodic_subscription();
        subscription.provider.last_automatic_failure_unix = Some(5_000);
        subscription.provider.metadata.values.insert(
            ProviderKey::SubscriptionAutoUpdateOpenEnable,
            Secret::new("1"),
        );
        let subscriptions = [subscription];
        let now = Instant::now();
        let mut restarted = AutomaticRefresh::default();
        restarted.start_launch(&subscriptions);
        for include_launch in [false, true] {
            assert_eq!(
                restarted.next(&subscriptions, None, now, 5_001, include_launch),
                Some(("fixture", Duration::from_secs(3_599)))
            );
            assert_eq!(
                restarted.next(&subscriptions, None, now, 8_600, include_launch),
                Some(("fixture", Duration::ZERO))
            );
        }
        assert_eq!(restarted.periodic_deadline(&subscriptions), Some(8_600));
    }
}
