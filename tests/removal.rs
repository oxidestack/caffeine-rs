//! Ports of Caffeine 3.1.8's removal-path tests over the key-only `BoundedCache`:
//! `CacheTest.invalidate_*` / `invalidateAll_*` (CacheTest.java:719-790), `EvictionTest.evict`
//! (EvictionTest.java:109), `evict_evictionListenerFails`' `setMaximum(0)` (:276), and the
//! `remove`/`invalidateAll` cases of the weighted section (:505-540) as protected-size bookkeeping.

use std::sync::{Arc, Mutex};

use caffeine::{BoundedCache, RemovalCause};

type Log = Arc<Mutex<Vec<(u64, RemovalCause)>>>;

fn cache_with_logs(max: u64, keys: u64) -> (BoundedCache<u64>, Log, Log) {
    let evictions: Log = Arc::default();
    let removals: Log = Arc::default();
    let e = evictions.clone();
    let mut cache = BoundedCache::with_eviction_listener(max, move |k, c| e.lock().unwrap().push((*k, c)));
    let r = removals.clone();
    cache.set_removal_listener(move |k, c| r.lock().unwrap().push((*k, c)));
    for k in 0..keys {
        cache.put(k);
    }
    (cache, evictions, removals)
}

fn sorted(log: &Log) -> Vec<(u64, RemovalCause)> {
    let mut v = log.lock().unwrap().clone();
    v.sort_by_key(|(k, _)| *k);
    v
}

#[test]
fn invalidate_absent() {
    let (mut cache, ev, rm) = cache_with_logs(100, 10);
    cache.invalidate(&999);
    assert!(!cache.remove(&999));
    assert_eq!(cache.len(), 10);
    assert!(ev.lock().unwrap().is_empty() && rm.lock().unwrap().is_empty());
}

#[test]
fn invalidate_present_first_middle_last() {
    let (mut cache, ev, rm) = cache_with_logs(100, 10);
    for k in [0u64, 5, 9] {
        cache.invalidate(&k);
        assert!(!cache.contains(&k));
    }
    assert_eq!(cache.len(), 7);
    assert_eq!(sorted(&rm), vec![(0, RemovalCause::Explicit), (5, RemovalCause::Explicit), (9, RemovalCause::Explicit)]);
    // evictionListener sees evictions only, never explicit removals (notifyEviction guards on wasEvicted()).
    assert!(ev.lock().unwrap().is_empty());
}

#[test]
fn invalidate_all_empty_partial_full() {
    let (mut empty, _, rm) = cache_with_logs(100, 0);
    empty.invalidate_all();
    empty.invalidate_all_of(std::iter::empty());
    assert!(empty.is_empty() && rm.lock().unwrap().is_empty());

    let (mut cache, ev, rm) = cache_with_logs(100, 10);
    cache.invalidate_all_of(&[2u64, 4, 6, 77]);
    assert_eq!(cache.len(), 7);
    assert_eq!(sorted(&rm).len(), 3);

    rm.lock().unwrap().clear();
    cache.invalidate_all();
    assert!(cache.is_empty());
    let expected: Vec<_> = [0u64, 1, 3, 5, 7, 8, 9].iter().map(|k| (*k, RemovalCause::Explicit)).collect();
    assert_eq!(sorted(&rm), expected);
    assert!(ev.lock().unwrap().is_empty());
    // The cache is reusable afterwards.
    cache.put(1);
    assert_eq!(cache.len(), 1);
}

/// EvictionTest.evict for maximumSize ZERO / ONE / FULL: after adding `absent` keys the cache holds
/// the maximum, and the SIZE notifications are exactly the keys that left.
#[test]
fn evict_notifies_exactly_the_evicted_keys() {
    for max in [0u64, 1, 10] {
        let (mut cache, ev, rm) = cache_with_logs(max, 0);
        let mut returned = Vec::new();
        for k in 0..30u64 {
            returned.extend(cache.put_evicting(k));
            assert!(cache.len() as u64 <= max);
        }
        let mut gone: Vec<u64> = (0..30).filter(|k| !cache.contains(k)).collect();
        gone.sort();
        let mut notified: Vec<u64> = sorted(&ev).into_iter().map(|(k, c)| {
            assert_eq!(c, RemovalCause::Size);
            k
        }).collect();
        notified.sort();
        returned.sort();
        assert_eq!(notified, gone);
        assert_eq!(returned, gone);
        assert_eq!(sorted(&rm).len(), gone.len());
        assert_eq!(cache.len() as u64, max);
    }
}

/// EvictionTest.evict_evictionListenerFails: setMaximum(0) evicts every entry, one notification each.
#[test]
fn set_maximum_zero_evicts_everything() {
    let (mut cache, ev, _) = cache_with_logs(50, 50);
    let evicted = cache.set_maximum(0);
    assert!(cache.is_empty());
    assert_eq!(evicted.len(), 50);
    assert_eq!(ev.lock().unwrap().len(), 50);
    assert_eq!(cache.maximum(), 0);
}

#[test]
fn set_maximum_shrinks_and_grows() {
    let (mut cache, ev, _) = cache_with_logs(100, 100);
    assert_eq!(cache.set_maximum(40).len(), 60);
    assert_eq!(cache.len(), 40);
    assert_eq!(ev.lock().unwrap().len(), 60);
    assert!(cache.set_maximum(40).is_empty());
    assert!(cache.set_maximum(200).is_empty());
    for k in 1000..1160u64 {
        cache.put(k);
    }
    assert_eq!(cache.len(), 200);
}

/// EvictionTest.remove / removeConditionally: removing entries leaves no ghost capacity, including
/// entries promoted to the protected segment (their protected weight is given back).
#[test]
fn remove_frees_capacity_and_protected_weight() {
    let (mut cache, ev, _) = cache_with_logs(100, 0);
    for k in 0..100u64 {
        cache.put(k);
    }
    // Promote a block into the protected segment.
    for _ in 0..3 {
        for k in 0..80u64 {
            cache.get(&k);
        }
    }
    for k in 0..60u64 {
        assert!(cache.remove(&k));
    }
    assert_eq!(cache.len(), 40);
    // No ghosts: refilling to the bound evicts nothing while it fits.
    for k in 1000..1060u64 {
        assert!(cache.put_evicting(k).is_empty(), "ghost capacity evicted a live entry at {k}");
    }
    assert_eq!(cache.len(), 100);
    assert!(ev.lock().unwrap().is_empty());
    // Over the bound again: exactly one eviction per extra put.
    for k in 2000..2010u64 {
        assert_eq!(cache.put_evicting(k).len(), 1);
    }
    assert_eq!(cache.len(), 100);
}

#[test]
fn size_bound_with_interleaved_removals() {
    let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(64);
    let mut x = 0x2545F491u32;
    for i in 0..20_000u64 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let k = u64::from(x % 300);
        match i % 3 {
            0 => {
                cache.remove(&k);
            }
            1 => {
                cache.get(&k);
            }
            _ => {
                cache.put(k);
            }
        }
        assert!(cache.len() <= 64);
    }
}
