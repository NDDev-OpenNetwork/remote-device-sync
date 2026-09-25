//! Numeric snapshots only. Metric names and the two path labels are static
//! product metadata, never peer IDs, addresses, filenames or user strings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Counter names end in `_total`; every other sample is a gauge. Producers
/// must return bounded, nonblocking in-memory observations, without owning I/O
/// resources. Concurrent counters are individual observations, not a transaction.
pub type Snapshot = BTreeMap<&'static str, u64>;

pub(super) const MAX_SAMPLES: usize = 256;
pub(super) const MAX_RESPONSE: usize = 64 * 1024;

pub(super) fn render(samples: Snapshot) -> Option<String> {
    if samples.len() > MAX_SAMPLES {
        return None;
    }
    let mut output = String::new();
    let mut families = BTreeSet::new();
    for (name, value) in samples {
        let (family, labels) = name.split_once('{').unwrap_or((name, ""));
        if family.is_empty()
            || family.len() > 128
            || !family.starts_with("rds_")
            || !family
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !matches!(labels, "" | "via=\"direct\"}" | "via=\"relay\"}")
            || (labels.is_empty() && name.contains('{'))
        {
            return None;
        }
        if families.insert(family) {
            let kind = if family.ends_with("_total") {
                "counter"
            } else {
                "gauge"
            };
            writeln!(output, "# TYPE {family} {kind}").ok()?;
        }
        writeln!(output, "{name} {value}").ok()?;
        if output.len() > MAX_RESPONSE {
            return None;
        }
    }
    Some(output)
}
