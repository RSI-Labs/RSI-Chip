//! Bounded, deterministic RTL implementation-flow regression cache.
//!
//! The cache is keyed by source, selected top, and the complete logical
//! definition of the synthesis library. It is intentionally explicit and
//! dependency-free: agents can share it in process or wrap the typed key in a
//! distributed content-addressable store without guessing cache identity.

use crate::{pdk_liberty_sha256, run_flow_with_library, Flow};
use eda_core::library::CellLibrary;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FlowCacheKey {
    pub source_sha256: String,
    pub top: Option<String>,
    pub library_fingerprint: String,
}

impl FlowCacheKey {
    pub fn new(source: &str, top: Option<&str>, library: &CellLibrary) -> Self {
        Self {
            source_sha256: pdk_liberty_sha256(source),
            top: top.map(str::to_string),
            library_fingerprint: library_fingerprint(library),
        }
    }

    pub fn cache_id(&self) -> String {
        let top = self.top.as_deref().unwrap_or("");
        pdk_liberty_sha256(&format!(
            "eda-flow-cache-v1\0{}\0{}\0{}\0",
            self.source_sha256, top, self.library_fingerprint
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
}

#[derive(Clone, Debug)]
pub struct CachedFlow {
    pub flow: Flow,
    pub cache_hit: bool,
}

#[derive(Clone, Debug)]
pub struct FlowCache {
    capacity: usize,
    entries: HashMap<String, Flow>,
    order: VecDeque<String>,
    stats: FlowCacheStats,
}

impl FlowCache {
    pub fn new(capacity: usize) -> Result<Self, String> {
        if capacity == 0 {
            return Err("flow cache capacity must be nonzero".into());
        }
        Ok(Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
            stats: FlowCacheStats::default(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn stats(&self) -> FlowCacheStats {
        self.stats
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn contains(&self, key: &FlowCacheKey) -> bool {
        self.entries.contains_key(&key.cache_id())
    }

    pub fn get(&mut self, key: &FlowCacheKey) -> Option<Flow> {
        let id = key.cache_id();
        if let Some(order) = self.order.iter().position(|cached| *cached == id) {
            self.order.remove(order);
            self.order.push_back(id.clone());
        }
        if self.entries.contains_key(&id) {
            self.stats.hits += 1;
            self.entries.get(&id).cloned()
        } else {
            self.stats.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, key: &FlowCacheKey, flow: Flow) -> Result<(), String> {
        let id = key.cache_id();
        if let Some(order) = self.order.iter().position(|cached| *cached == id) {
            self.order.remove(order);
        }
        self.order.push_back(id.clone());
        self.entries.insert(id, flow);
        self.stats.inserts += 1;
        while self.entries.len() > self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
                self.stats.evictions += 1;
            } else {
                return Err("flow cache LRU order is inconsistent".into());
            }
        }
        Ok(())
    }

    pub fn get_or_insert_with(
        &mut self,
        key: &FlowCacheKey,
        producer: impl FnOnce() -> Result<Flow, String>,
    ) -> Result<CachedFlow, String> {
        let id = key.cache_id();
        if let Some(flow) = self.entries.get(&id).cloned() {
            if let Some(order) = self.order.iter().position(|cached| *cached == id) {
                self.order.remove(order);
            }
            self.order.push_back(id);
            self.stats.hits += 1;
            return Ok(CachedFlow {
                flow,
                cache_hit: true,
            });
        }
        self.stats.misses += 1;
        let flow = producer()?;
        self.insert(key, flow.clone())?;
        Ok(CachedFlow {
            flow,
            cache_hit: false,
        })
    }

    pub fn run(
        &mut self,
        source: &str,
        top: Option<&str>,
        library: &CellLibrary,
    ) -> Result<CachedFlow, String> {
        let key = FlowCacheKey::new(source, top, library);
        self.get_or_insert_with(&key, || run_flow_with_library(source, top, library))
    }
}

#[derive(Clone)]
pub struct SharedFlowCache {
    inner: Arc<Mutex<FlowCache>>,
}

impl SharedFlowCache {
    pub fn new(capacity: usize) -> Result<Self, String> {
        Ok(Self {
            inner: Arc::new(Mutex::new(FlowCache::new(capacity)?)),
        })
    }

    pub fn run(
        &self,
        source: &str,
        top: Option<&str>,
        library: &CellLibrary,
    ) -> Result<CachedFlow, String> {
        let key = FlowCacheKey::new(source, top, library);
        let mut cache = self
            .inner
            .lock()
            .map_err(|_| "flow cache mutex poisoned".to_string())?;
        cache.get_or_insert_with(&key, || run_flow_with_library(source, top, library))
    }

    pub fn get_or_insert_with(
        &self,
        key: &FlowCacheKey,
        producer: impl FnOnce() -> Result<Flow, String>,
    ) -> Result<CachedFlow, String> {
        let mut cache = self
            .inner
            .lock()
            .map_err(|_| "flow cache mutex poisoned".to_string())?;
        cache.get_or_insert_with(key, producer)
    }

    pub fn stats(&self) -> Result<FlowCacheStats, String> {
        self.inner
            .lock()
            .map(|cache| cache.stats())
            .map_err(|_| "flow cache mutex poisoned".to_string())
    }

    pub fn clear(&self) -> Result<(), String> {
        self.inner
            .lock()
            .map(|mut cache| cache.clear())
            .map_err(|_| "flow cache mutex poisoned".to_string())
    }

    pub fn with_stats(
        &self,
        operation: impl FnOnce(FlowCacheStats) -> Result<(), String>,
    ) -> Result<(), String> {
        let cache = self
            .inner
            .lock()
            .map_err(|_| "flow cache mutex poisoned".to_string())?;
        operation(cache.stats())
    }
}

pub fn library_fingerprint(library: &CellLibrary) -> String {
    let mut canonical = String::new();
    push_field(&mut canonical, "name", &library.name);
    push_usize(&mut canonical, "cells", library.cells.len());
    for cell in &library.cells {
        push_field(&mut canonical, "cell", &cell.name);
        push_usize(&mut canonical, "inputs", cell.inputs.len());
        for input in &cell.inputs {
            push_field(&mut canonical, "input", input);
        }
        push_field(&mut canonical, "output", &cell.output);
        push_field(&mut canonical, "function", &format!("{:?}", cell.func));
        push_usize(&mut canonical, "width", cell.width_sites);
        push_usize(&mut canonical, "arcs", cell.arcs.len());
        for (input, delay) in &cell.arcs {
            push_usize(&mut canonical, "arc-input", *input);
            push_field(&mut canonical, "arc-delay", &format!("{:?}", delay));
        }
        push_field(
            &mut canonical,
            "timing-tables",
            &format!("{:?}", cell.timing_tables),
        );
        push_field(&mut canonical, "seq", &format!("{:?}", cell.seq));

        let metadata = library.metadata(&cell.name);
        push_f64_map(&mut canonical, "input-cap", &metadata.input_capacitance_pf);
        push_f64_map(
            &mut canonical,
            "max-transition",
            &metadata.max_transition_ns,
        );
        push_f64_map(&mut canonical, "max-cap", &metadata.max_capacitance_pf);
        push_field(
            &mut canonical,
            "metadata",
            &format!(
                "area={:?}\0leakage={:?}\0leakage-states={:?}\0internal={:?}",
                metadata.area,
                metadata.leakage_power,
                metadata.leakage_states,
                metadata.internal_power
            ),
        );
    }
    pdk_liberty_sha256(&canonical)
}

fn push_field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push('\0');
    out.push_str(&value.len().to_string());
    out.push('\0');
    out.push_str(value);
    out.push('\0');
}

fn push_usize(out: &mut String, name: &str, value: usize) {
    push_field(out, name, &value.to_string());
}

fn push_f64_map(out: &mut String, name: &str, values: &HashMap<String, f64>) {
    let sorted: BTreeMap<_, _> = values.iter().collect();
    let text = sorted
        .iter()
        .map(|(key, value)| format!("{key}={value:?}"))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    push_field(out, name, &text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_flow;

    #[test]
    fn flow_cache_reuses_and_invalidates_inputs() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let library = CellLibrary::std();
        let key = FlowCacheKey::new(source, Some("top"), &library);
        let mut cache = FlowCache::new(2).unwrap();
        assert!(!cache.contains(&key));
        let first = cache.run(source, Some("top"), &library).unwrap();
        assert!(!first.cache_hit);
        let second = cache.run(source, Some("top"), &library).unwrap();
        assert!(second.cache_hit);
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(first.flow.verilog, second.flow.verilog);

        let changed = "module top(input a, output y); assign y = a; endmodule";
        let changed_key = FlowCacheKey::new(changed, Some("top"), &library);
        assert_ne!(key.cache_id(), changed_key.cache_id());
        let third = cache.run(changed, Some("top"), &library).unwrap();
        assert!(!third.cache_hit);
    }

    #[test]
    fn flow_cache_is_bounded_and_lru() {
        let library = CellLibrary::std();
        let mut cache = FlowCache::new(2).unwrap();
        let sources = [
            "module a(input x, output y); assign y = ~x; endmodule",
            "module b(input x, output y); assign y = x; endmodule",
            "module c(input x, output y); assign y = ~x; endmodule",
        ];
        for source in sources {
            cache.run(source, None, &library).unwrap();
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.stats().evictions, 1);
        assert!(!cache.contains(&FlowCacheKey::new(sources[0], None, &library)));
        assert!(cache.contains(&FlowCacheKey::new(sources[1], None, &library)));
        assert!(cache.contains(&FlowCacheKey::new(sources[2], None, &library)));
    }

    #[test]
    fn library_fingerprint_distinguishes_metadata() {
        let mut left = CellLibrary::std();
        let mut right = CellLibrary::std();
        left.name = "left".into();
        right.name = "right".into();
        assert_ne!(library_fingerprint(&left), library_fingerprint(&right));
        assert_eq!(library_fingerprint(&left), library_fingerprint(&left));
        assert!(!run_flow(
            "module top(input a, output y); assign y = ~a; endmodule",
            Some("top")
        )
        .unwrap()
        .verilog
        .is_empty());
    }
}
