//! Disposable bounded observation. Journals, snapshots and core recovery remain
//! authoritative; this view deliberately does not notice edits to old disk data
//! until eviction or explicit inactive invalidation.
use crate::{EventLine, RunObservation, RunStatusInfo};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;

pub(crate) const RETAIN_RECORDS: usize = 256;
pub(crate) const RETAIN_BYTES: usize = 512 * 1024;
const BATCH_RECORDS: usize = 128;
const BATCH_BYTES: usize = 256 * 1024;
const COLD_CACHE_ENTRIES: usize = 8;

#[derive(Default)]
pub(crate) struct Projection {
    events: VecDeque<EventLine>,
    bytes: usize,
    pub(crate) last_seq: u64,
    // Includes oversized records that could never fit a returned batch.
    omitted_through: u64,
    pub(crate) report_node: Option<String>,
    pub(crate) final_report: Option<String>,
    pub(crate) report_error: Option<String>,
    #[cfg(test)]
    processed: usize,
}

impl Projection {
    pub(crate) fn seed(base: &Path, run_id: &str) -> anyhow::Result<Self> {
        let mut view = Self::default();
        trollm_core::EventLog::visit(
            &base.join("runs").join(run_id).join("events.jsonl"),
            Some(run_id),
            |value, line| view.committed(value, line),
        )?;
        Ok(view)
    }

    pub(crate) fn committed(&mut self, value: &Value, line: &str) {
        let seq = value["seq"].as_u64().expect("committed validated event");
        self.last_seq = seq;
        #[cfg(test)]
        {
            self.processed += 1;
        }
        if value["type"] == "transition" && value["key"] == "onVerified" {
            if let Some(node) = value["from"].as_str() {
                self.report_node = Some(node.to_owned());
            }
        }
        // Every retained record must fit one batch, allowing forward progress
        // even when one committed record exceeds the observation byte budget.
        if line.len() > BATCH_BYTES {
            self.omitted_through = seq;
            self.events.clear();
            self.bytes = 0;
            return;
        }
        self.bytes += line.len();
        self.events.push_back(EventLine {
            seq,
            event_type: value["type"].as_str().unwrap().to_owned(),
            json: line.to_owned(),
        });
        while self.events.len() > RETAIN_RECORDS || self.bytes > RETAIN_BYTES {
            let old = self.events.pop_front().unwrap();
            self.bytes -= old.json.len();
            self.omitted_through = old.seq;
        }
    }

    pub(crate) fn batch(&self, status: RunStatusInfo, from: u64) -> RunObservation {
        let omitted = from < self.omitted_through;
        let mut next = from.max(self.omitted_through);
        let mut bytes = 0;
        let events: Vec<_> = self
            .events
            .iter()
            .filter(|event| event.seq > from)
            .take_while(|event| {
                if bytes + event.json.len() > BATCH_BYTES {
                    return false;
                }
                bytes += event.json.len();
                true
            })
            .take(BATCH_RECORDS)
            .cloned()
            .collect();
        if let Some(last) = events.last() {
            next = last.seq;
        }
        let has_more = self.events.iter().any(|event| event.seq > next);
        if !has_more {
            next = next.max(self.last_seq);
        }
        RunObservation {
            status,
            events,
            next_seq: next,
            older_events_omitted: omitted,
            has_more,
        }
    }
}

#[derive(Default)]
pub(crate) struct ColdCache(VecDeque<(String, Projection, RunStatusInfo)>);
impl ColdCache {
    pub(crate) fn get(&self, id: &str) -> Option<(&Projection, &RunStatusInfo)> {
        self.0
            .iter()
            .find(|entry| entry.0 == id)
            .map(|entry| (&entry.1, &entry.2))
    }
    pub(crate) fn remove(&mut self, id: &str) {
        self.0.retain(|entry| entry.0 != id);
    }
    pub(crate) fn insert(&mut self, id: String, view: Projection, status: RunStatusInfo) {
        self.remove(&id);
        self.0.push_back((id, view, status));
        while self.0.len() > COLD_CACHE_ENTRIES {
            self.0.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunPhaseApi;
    use serde_json::json;
    fn status() -> RunStatusInfo {
        RunStatusInfo {
            run_id: "synthetic".into(),
            phase: RunPhaseApi::Succeeded,
            worker_active: false,
            activity: None,
            last_seq: 0,
            final_report: None,
        }
    }
    #[test]
    fn long_projection_and_idle_batches_are_bounded() {
        let mut view = Projection::default();
        for seq in 1..=10_000 {
            let value =
                json!({"seq": seq, "type": "transition", "key":"onVerified", "from":"report"});
            view.committed(&value, &serde_json::to_string(&value).unwrap());
        }
        assert_eq!(view.processed, 10_000);
        assert_eq!(view.events.len(), RETAIN_RECORDS);
        assert!(view.bytes <= RETAIN_BYTES);
        assert_eq!(view.report_node.as_deref(), Some("report"));
        let batch = view.batch(status(), 0);
        assert!(batch.older_events_omitted && batch.has_more);
        assert_eq!(batch.events.len(), BATCH_RECORDS);
        for _ in 0..100 {
            assert!(view.batch(status(), 10_000).events.is_empty());
        }
        assert_eq!(view.processed, 10_000);
    }
    #[test]
    fn oversized_record_advances_even_an_empty_batch() {
        let mut view = Projection::default();
        view.committed(
            &json!({"seq":1,"type":"model_call"}),
            &"x".repeat(BATCH_BYTES + 1),
        );
        let batch = view.batch(status(), 0);
        assert!(batch.events.is_empty() && batch.older_events_omitted);
        assert_eq!(batch.next_seq, 1);
        assert!(!batch.has_more);
    }
    #[test]
    fn report_source_outlives_ring_and_byte_bound() {
        let mut view = Projection::default();
        view.committed(
            &json!({"seq":1,"type":"transition","key":"onVerified","from":"final"}),
            "{}",
        );
        for seq in 2..20 {
            view.committed(
                &json!({"seq":seq,"type":"model_call"}),
                &"x".repeat(BATCH_BYTES),
            );
        }
        assert!(view.bytes <= RETAIN_BYTES);
        assert_eq!(view.report_node.as_deref(), Some("final"));
    }
    #[test]
    fn inactive_cache_evicts_and_invalidates() {
        let mut cache = ColdCache::default();
        for id in 0..20 {
            cache.insert(id.to_string(), Projection::default(), status());
        }
        assert_eq!(cache.0.len(), COLD_CACHE_ENTRIES);
        assert!(cache.get("0").is_none());
        cache.remove("19");
        assert!(cache.get("19").is_none());
    }
}
