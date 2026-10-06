//! Rank existing paths by RTT without making a relay permanently secondary.
use std::time::Duration;

use iroh::endpoint::transports::{PathSelection, PathSelectionContext, PathSelector};

const SWITCH_GAIN: Duration = Duration::from_millis(5);

#[derive(Debug)]
pub(super) struct LatencySelector;

impl PathSelector for LatencySelector {
    fn refresh_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let paths: Vec<_> = ctx
            .paths()
            .filter_map(|path| {
                let rtt = path.stats()?.rtt;
                let progress = path
                    .congestion_state()
                    .and_then(crate::ack_progress::snapshot);
                if progress.is_some_and(|state| state.needs_probe()) {
                    path.ping();
                }
                Some((path, rtt, progress))
            })
            .collect();
        for (failed, rtt, progress) in &paths {
            if !progress.is_some_and(|state| state.stalled(*rtt)) {
                continue;
            }
            for (fallback, other_rtt, state) in &paths {
                if state
                    .is_some_and(|state| state.confirmed(*other_rtt) && !state.stalled(*other_rtt))
                    && failed.abandon_with_fallback(fallback)
                {
                    tracing::warn!(target:"rds_net::path_policy",
                        pending_ack_age_ms=?progress.and_then(|state| state.pending_age()).map(|age| age.as_millis()),
                        fallback_ack_age_ms=?state.and_then(|state| state.confirmation_age()).map(|age| age.as_millis()),
                        path_rtt_ms=rtt.as_millis(), fallback_rtt_ms=other_rtt.as_millis(),
                        "unresponsive path retired with a confirmed sibling; reliable streams retained");
                    break;
                }
            }
        }
        let has_confirmed = paths.iter().any(|(_, rtt, state)| {
            state.is_some_and(|state| state.confirmed(*rtt) && !state.stalled(*rtt))
        });
        let choice = choose(paths.into_iter().filter_map(|(path, rtt, state)| {
            if state.is_some_and(|state| state.stalled(rtt))
                || (has_confirmed && state.is_some_and(|state| !state.confirmed(rtt)))
            {
                return None;
            }
            let current = Some(path.network_path()) == ctx.current();
            Some((path, rtt, current))
        }));
        let mut selection = PathSelection::none();
        if let Some(path) = choice {
            selection.set(&path);
        }
        selection
    }
}

fn choose<T>(paths: impl Iterator<Item = (T, Duration, bool)>) -> Option<T> {
    let mut best: Option<(T, Duration)> = None;
    let mut current: Option<Duration> = None;
    for (path, rtt, selected) in paths {
        if selected && current.is_none_or(|old| rtt < old) {
            current = Some(rtt);
        }
        if best.as_ref().is_none_or(|(_, old)| rtt < *old) {
            best = Some((path, rtt));
        }
    }
    let (path, rtt) = best?;
    if current.is_none_or(|old| old.saturating_sub(rtt) >= SWITCH_GAIN) {
        Some(path)
    } else {
        // The Iroh selector contract interprets an empty selection as keep current.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone, Copy)]
    enum Route {
        Direct,
        Relay,
    }

    #[test]
    fn usable_direct_path_does_not_hide_a_faster_relay() {
        let paths = [
            (Route::Direct, Duration::from_millis(170), true),
            (Route::Relay, Duration::from_millis(65), false),
        ];
        assert_eq!(choose(paths.into_iter()), Some(Route::Relay));
        assert_eq!(choose(paths.into_iter().rev()), Some(Route::Relay));
    }

    #[test]
    fn stickiness_ties_missing_current_and_extreme_values_are_explicit() {
        let ms = Duration::from_millis;
        assert_eq!(
            choose([(Route::Direct, ms(69), true), (Route::Relay, ms(65), false)].into_iter()),
            None
        );
        assert_eq!(
            choose([(Route::Direct, ms(70), true), (Route::Relay, ms(65), false)].into_iter()),
            Some(Route::Relay)
        );
        assert_eq!(
            choose([(Route::Direct, ms(65), true), (Route::Relay, ms(65), false)].into_iter()),
            None
        );
        assert_eq!(
            choose([(Route::Relay, ms(65), false)].into_iter()),
            Some(Route::Relay)
        );
        assert_eq!(
            choose(
                [
                    (Route::Direct, Duration::MAX, true),
                    (Route::Relay, Duration::MAX, false)
                ]
                .into_iter()
            ),
            None
        );
        assert_eq!(choose(std::iter::empty::<(Route, Duration, bool)>()), None);
    }
}
