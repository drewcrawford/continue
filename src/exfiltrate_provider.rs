// SPDX-License-Identifier: MIT OR Apache-2.0

//! Exposes the continuation registry through exfiltrate's `snapshot` command.
//!
//! Call [`install`] once, after `exfiltrate::begin()`.

use exfiltrate::provider::{Provider, ProviderResult, Row, SnapshotRequest};
use wasm_lite_std::time::Instant;

use crate::registry;

/// Registers the `continuations` subsystem. Idempotent.
///
/// ```no_run
/// exfiltrate::begin();
/// r#continue::exfiltrate_provider::install();
/// ```
pub fn install() {
    exfiltrate::provider::add_provider(Continuations);
}

struct Continuations;

impl Provider for Continuations {
    fn subsystem(&self) -> &'static str {
        "continuations"
    }

    fn description(&self) -> &'static str {
        "outstanding continuations: age, creation site, and whether anyone is awaiting them"
    }

    fn snapshot(&self, request: &SnapshotRequest<'_>) -> ProviderResult {
        let Some(entries) = registry::entries() else {
            return ProviderResult::Busy;
        };
        let stats = registry::stats();

        let wanted: Option<u64> = match request.selector() {
            Some(selector) => match selector.parse::<u64>() {
                Ok(id) => Some(id),
                Err(_) => {
                    return ProviderResult::Unavailable(format!(
                        "--id must be a continuation id; got {selector:?}"
                    ));
                }
            },
            None => None,
        };

        let now = Instant::now();
        let mut rows = Vec::new();
        for entry in entries {
            if request.should_stop() {
                return ProviderResult::Partial(rows, "deadline".to_string());
            }
            if let Some(id) = wanted
                && entry.seq != id
            {
                continue;
            }
            rows.push(
                Row::new()
                    .support("id", entry.seq)
                    // The same token the logwise records carry, so a stuck
                    // continuation joins to the task awaiting it. `0` when no
                    // logwise runtime was installed to mint one.
                    .support("context", entry.context)
                    .support("outcome", entry.outcome.name())
                    .support(
                        "age_ms",
                        now.duration_since(entry.created).as_millis() as u64,
                    )
                    .support("polled", entry.polled)
                    // A source location from this build, not caller data.
                    .support("created_at_file", entry.created_at_file)
                    .support("created_at_line", u64::from(entry.created_at_line)),
            );
        }

        if stats.dropped > 0 {
            return ProviderResult::Partial(
                rows,
                format!(
                    "the registry has dropped {} continuation(s) to stay within its capacity \
                     of {}; raise CONTINUE_REGISTRY_CAPACITY to keep more",
                    stats.dropped, stats.capacity
                ),
            );
        }
        ProviderResult::Rows(rows)
    }
}
