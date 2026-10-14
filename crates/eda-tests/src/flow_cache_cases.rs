//! Bounded RTL implementation-flow cache regressions.

use crate::CaseList;
use eda_api::{CellLibrary, FlowCache, SharedFlowCache};
use std::sync::Arc;

pub fn register(c: &mut CaseList) {
    for index in 0_u32..100 {
        c.add(format!("flow_cache_source_{index:03}"), move || {
            let source = format!(
                "module top(input [{index}:0] a, output [{index}:0] y); assign y = ~a; endmodule"
            );
            let library = CellLibrary::std();
            let mut cache = FlowCache::new(8)?;
            let first = cache.run(&source, Some("top"), &library)?;
            let second = cache.run(&source, Some("top"), &library)?;
            if first.cache_hit
                || !second.cache_hit
                || first.flow.verilog != second.flow.verilog
                || cache.stats().hits != 1
                || cache.stats().misses != 1
                || cache.stats().inserts != 1
                || cache.len() != 1
            {
                Err(format!(
                    "index={} stats={:?} first-hit={} second-hit={}",
                    index,
                    cache.stats(),
                    first.cache_hit,
                    second.cache_hit
                ))
            } else {
                Ok(())
            }
        });
    }

    c.add("flow_cache_invalid_source_not_inserted", || {
        let mut cache = FlowCache::new(4)?;
        let error = cache.run(
            "module top(input a); ghost u(a); endmodule",
            None,
            &CellLibrary::std(),
        );
        if error.is_ok() || !cache.is_empty() || cache.stats().misses != 1 {
            Err(format!("stats={:?}", cache.stats()))
        } else {
            Ok(())
        }
    });

    c.add("flow_cache_shared_by_concurrent_agents", || {
        let cache = SharedFlowCache::new(4)?;
        let source: &'static str = "module top(input a, b, output y); assign y = a & b; endmodule";
        let library = Arc::new(CellLibrary::std());
        let workers = (0..8)
            .map(|_| {
                let cache = cache.clone();
                let library = Arc::clone(&library);
                std::thread::spawn(move || cache.run(source, Some("top"), &library))
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().map_err(|_| "cache worker panicked")??;
        }
        cache.with_stats(|stats| {
            if stats.misses != 1 || stats.hits != 7 || stats.inserts != 1 || stats.evictions != 0 {
                Err(format!("stats={stats:?}"))
            } else {
                Ok(())
            }
        })
    });
}
