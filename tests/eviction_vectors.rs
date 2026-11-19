//! The victim sequence checked against the real `caffeine-3.1.8.jar`.
//!
//! `upstream-tools/EvictionVectorGen.java` drives a real Caffeine cache built as Infinispan's
//! `BoundedSegmentedDataContainer` builds it (`maximumSize`, maintenance on the calling thread, an
//! eviction listener) through `asMap().compute` / `get` / `computeIfPresent(-> null)` and
//! `policy().eviction().setMaximum`, and records what the listener saw per operation
//! (`tests/vectors/eviction_*.txt`). This replays each operation on [`BoundedCache`] and compares.
//!
//! A vector is only reproducible while the admission's hash-flooding coin is never tossed (upstream
//! tosses `ThreadLocalRandom`), so each replay also asserts the port never reached it.

use std::sync::{Arc, Mutex};

use caffeine::BoundedCache;

fn replay(name: &str) {
    let path = format!("{}/tests/vectors/eviction_{name}.txt", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut lines = text.lines().enumerate();

    let (_, header) = lines.next().expect("max line");
    let max: u64 = header.strip_prefix("max ").expect("max line").parse().unwrap();
    let seen: Arc<Mutex<Vec<i32>>> = Arc::default();
    let log = seen.clone();
    let mut cache: BoundedCache<i32> =
        BoundedCache::with_eviction_listener(max, move |k, _| log.lock().unwrap().push(*k));

    for (n, line) in lines {
        let mut fields = line.split(' ');
        let op = fields.next().unwrap();
        let rest: Vec<i32> = fields.map(|f| f.parse().unwrap()).collect();
        if op == "end" {
            let mut resident: Vec<i32> = rest.iter().copied().filter(|k| cache.contains(k)).collect();
            resident.sort_unstable();
            assert_eq!(resident, rest, "{name}: resident keys at the end");
            assert_eq!(cache.len(), rest.len(), "{name}: resident count at the end");
            break;
        }
        let (key, expected) = (rest[0], &rest[1..]);
        match op {
            "p" => {
                cache.put(key);
            }
            "g" => {
                cache.get(&key);
            }
            "r" => {
                cache.remove(&key);
            }
            "m" => {
                cache.set_maximum(key as u64);
            }
            other => panic!("{name}:{}: unknown op {other}", n + 1),
        }
        let got = std::mem::take(&mut *seen.lock().unwrap());
        assert_eq!(got, expected, "{name}:{}: evicted by `{line}`", n + 1);
    }
    assert_eq!(cache.admission_coin_flips(), 0, "{name}: the vector depends on upstream's random admission");
}

#[test]
fn tiny() {
    replay("tiny");
}

#[test]
fn uniform() {
    replay("uniform");
}

#[test]
fn read_bursts() {
    replay("read_bursts");
}

#[test]
fn skewed() {
    replay("skewed");
}

#[test]
fn scan() {
    replay("scan");
}

#[test]
fn resize() {
    replay("resize");
}
