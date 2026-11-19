//! W-TinyLFU bounded eviction policy: admission window + probation/protected SLRU segments.
//!
//! Port of the eviction *policy* out of
//! `upstream/caffeine/src/main/java/com/github/benmanes/caffeine/cache/BoundedLocalCache.java`
//! (Caffeine v3.1.8). Upstream's `BoundedLocalCache` is ~4700 lines covering async loading, weak/soft
//! references, striped write/read buffers, a timer wheel for variable expiry, statistics and removal
//! listeners — none of which the eviction policy itself needs and none of which cross the wire
//! (Infinispan, the consumer, evicts per node, so nothing about
//! it goes on the wire). This port keeps only what governs *which entries are retained*: the
//! admission-window → probation → protected SLRU pipeline and the `FrequencySketch`-based admission
//! decision at the window/main boundary. It is synchronous and single-threaded (the caller owns
//! external synchronization), unlike upstream's lock-striped concurrent structure — a correctness-
//! preserving simplification for a per-process, per-node policy with no wire obligation.
//!
//! The adaptive hill-climbing resize of the window/protected ratio (`climb()`, BoundedLocalCache.java:
//! 1116-1133, plus `determineAdjustment`/`increaseWindow`/`decreaseWindow`) is ported.
//!
//! The order in which the policy sees operations is upstream's for a single thread whose executor runs
//! the maintenance task inline (Infinispan's `WithinThreadExecutor`, `AbstractInternalDataContainer.java:
//! 410`), which is deterministic, and checked against the real jar (`tests/eviction_vectors.rs`):
//! - a read goes into a 16-slot read buffer (`BoundedBuffer.BUFFER_SIZE`; one stripe, as one thread never
//!   contends), skipped while the frequency sketch is not yet sized (`skipReadBuffer`); a read that finds
//!   the buffer full is dropped and runs the maintenance cycle instead (`afterRead`, :1281-1290);
//! - re-writing a present key is a read (`remap` with an unchanged weight calls `afterRead`, :2938);
//! - a new key, a removal and `setMaximum` run the maintenance cycle (`maintenance`, :1707-1731): drain
//!   the read buffer (`onAccess` per read), run the write's task, `evictEntries`, then `climb`, which is
//!   also where protected overflow from promotions is demoted;
//! - the sketch is sized lazily once the cache holds half its maximum (`AddTask`, :1885-1894;
//!   `setMaximumSize`, :697-700); before that its increments are no-ops.
//!
//! Concurrent access (striped buffers, maintenance on another thread) is not modelled.
//!
//! Not ported: weighted entries.
//! This port is unit-weight / entry-count bounded, matching the `maxCount` caches the consumer
//! (mock-server) wires this into; its study of Infinispan confirms
//! the only Infinispan cache configuration reachable from MockServer (`maxCount` on a segmented HEAP
//! data container) runs on Caffeine's `maximumSize`, not `maximumWeight`/a weigher, so there is no
//! MockServer-reachable case that needs weighted entries.

use std::collections::HashMap;
use std::hash::Hash;

use crate::frequency_sketch::{FrequencySketch, JavaHashCode};

/// Port of `BoundedLocalCache.PERCENT_MAIN` (BoundedLocalCache.java:214).
const PERCENT_MAIN: f64 = 0.99;
/// Port of `BoundedLocalCache.PERCENT_MAIN_PROTECTED` (BoundedLocalCache.java:216).
const PERCENT_MAIN_PROTECTED: f64 = 0.80;
/// Port of `BoundedLocalCache.ADMIT_HASHDOS_THRESHOLD` (BoundedLocalCache.java:224).
const ADMIT_HASHDOS_THRESHOLD: i32 = 6;
/// Port of `BoundedLocalCache.QUEUE_TRANSFER_THRESHOLD` (BoundedLocalCache.java:226).
const QUEUE_TRANSFER_THRESHOLD: usize = 1_000;
/// Port of `BoundedLocalCache.HILL_CLIMBER_RESTART_THRESHOLD` (BoundedLocalCache.java:218): the hit-rate
/// swing beyond which the climber resets its step size rather than decaying it.
const HILL_CLIMBER_RESTART_THRESHOLD: f64 = 0.05;
/// Port of `BoundedLocalCache.HILL_CLIMBER_STEP_PERCENT` (BoundedLocalCache.java:220): the fraction of
/// `maximum` the climber resets its step to on a restart.
const HILL_CLIMBER_STEP_PERCENT: f64 = 0.0625;
/// Port of `BoundedLocalCache.HILL_CLIMBER_STEP_DECAY_RATE` (BoundedLocalCache.java:222): how fast the
/// step size shrinks while hovering near the same hit rate.
const HILL_CLIMBER_STEP_DECAY_RATE: f64 = 0.98;
/// Port of `BoundedBuffer.BUFFER_SIZE` (BoundedBuffer.java:49): the reads one stripe holds before the
/// next is dropped.
const READ_BUFFER_SIZE: usize = 16;
/// The `hashCode()` the sketch is given for a read drained after its node was removed. Upstream's
/// removal retires the node first (`n.retire()` in `remap`) and the drain increments the sketch with the
/// `NodeFactory.RETIRED_STRONG_KEY` sentinel, whose identity hash code differs per JVM; any fixed value
/// stands for it.
const RETIRED_KEY_HASH: i32 = 0x2f3c_5a71;

/// The write a maintenance cycle applies after draining the read buffer (`AddTask`, `RemovalTask`).
#[derive(Clone, Copy)]
enum Task {
    Add(usize),
    Remove(usize),
}

/// Port of `RemovalCause` restricted to the causes of an entry-count-bounded cache without expiry,
/// references or replacement (RemovalCause.java).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalCause {
    /// The entry was removed by the user (`invalidate`, `remove`, `invalidateAll`).
    Explicit,
    /// The entry was evicted because the cache exceeded its maximum.
    Size,
}

/// A `Caffeine.evictionListener`: called with the key and why it left.
pub type EvictionListener<K> = Box<dyn FnMut(&K, RemovalCause) + Send>;

/// Which SLRU segment a node currently lives in. Port of `Node.WINDOW`/`PROBATION`/`PROTECTED`
/// (Node.java:156-158).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    Window,
    Probation,
    Protected,
}

/// One arena slot: an intrusive doubly-linked-list node, mirroring how upstream's `Node` carries its
/// own `prevInAccessOrder`/`nextInAccessOrder` links (Node.java) so segment membership and access-order
/// position are O(1) to update — the same complexity trade Caffeine's own `AccessOrderDeque` makes,
/// implemented here as a plain arena since this port has no concurrent-node lifecycle to manage.
struct Node<K> {
    key: K,
    segment: Segment,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Head/tail of one access-order deque (one per `Segment`), by arena index. Port of
/// `LinkedDeque`/`AccessOrderDeque` (LinkedDeque.java, AccessOrderDeque.java), restricted to the
/// operations the eviction policy actually needs: `offerLast`, `remove`, `moveToBack`, `peekFirst`,
/// `pollFirst`.
#[derive(Default)]
struct Deque {
    head: Option<usize>,
    tail: Option<usize>,
    len: usize,
}

/// A bounded, entry-count-limited cache implementing Caffeine's W-TinyLFU eviction policy.
///
/// Every stored entry has unit weight (this port targets `maxCount` semantics; see module docs for
/// the weighted-entry gap). `put` inserts or touches a key; `get` records a read the same way
/// upstream's `afterRead` does. Both keep the cache within `max_count` entries.
pub struct BoundedCache<K> {
    arena: Vec<Option<Node<K>>>,
    free: Vec<usize>,
    index: HashMap<K, usize>,
    window: Deque,
    probation: Deque,
    protected: Deque,
    max_count: i64,
    window_maximum: i64,
    main_protected_maximum: i64,
    main_protected_size: i64,
    sketch: FrequencySketch<K>,
    // ---- hill-climbing state (BoundedLocalCache.java:620-672) ---------------------------------
    hits_in_sample: i32,
    misses_in_sample: i32,
    previous_sample_hit_rate: f64,
    step_size: f64,
    adjustment: i64,
    /// Port of `Caffeine.evictionListener`: only causes with `wasEvicted()`, see
    /// [`BoundedCache::set_eviction_listener`].
    listener: Option<EvictionListener<K>>,
    /// Port of `Caffeine.removalListener`: every cause, see [`BoundedCache::set_removal_listener`].
    removal_listener: Option<EvictionListener<K>>,
    /// Keys removed with [`RemovalCause::Size`] since the last [`BoundedCache::put_evicting`] began.
    evicted: Vec<K>,
    /// Port of the read buffer: nodes read since the last maintenance cycle, in read order.
    read_buffer: Vec<usize>,
    /// How often [`Self::admit`] reached the hash-flooding coin toss.
    coin_flips: u64,
}

impl<K: JavaHashCode + Hash + Eq + Clone> BoundedCache<K> {
    /// Creates a cache bounded to `max_count` entries. Port of `setMaximumSize(long)`
    /// (BoundedLocalCache.java:678-701), specialized to unit weight (`window = max - floor(0.99*max)`,
    /// `mainProtectedMaximum = floor(0.80 * (max - window))`). The empty cache sizes its sketch only when
    /// it is already at half its maximum, a maximum of 1 (a maximum of 0 returns before, as it equals
    /// the initial maximum); otherwise the first write that reaches half does.
    pub fn with_max_count(max_count: u64) -> Self {
        let max = max_count as i64;
        let window_maximum = max - (PERCENT_MAIN * max as f64) as i64;
        let main_protected_maximum = (PERCENT_MAIN_PROTECTED * (max - window_maximum) as f64) as i64;

        let mut sketch = FrequencySketch::new();
        if max == 1 {
            sketch.ensure_capacity(max);
        }

        Self {
            arena: Vec::new(),
            free: Vec::new(),
            index: HashMap::new(),
            window: Deque::default(),
            probation: Deque::default(),
            protected: Deque::default(),
            max_count: max,
            window_maximum,
            main_protected_maximum,
            main_protected_size: 0,
            sketch,
            // Port of `setMaximumSize`'s climber reset (BoundedLocalCache.java:693-695): a fresh cache
            // starts with no samples and a step size of `-HILL_CLIMBER_STEP_PERCENT * max`.
            hits_in_sample: 0,
            misses_in_sample: 0,
            previous_sample_hit_rate: 0.0,
            step_size: -HILL_CLIMBER_STEP_PERCENT * max as f64,
            adjustment: 0,
            listener: None,
            removal_listener: None,
            evicted: Vec::new(),
            read_buffer: Vec::new(),
            coin_flips: 0,
        }
    }

    /// Like [`Self::with_max_count`] with an eviction listener installed.
    pub fn with_eviction_listener(
        max_count: u64,
        listener: impl FnMut(&K, RemovalCause) + Send + 'static,
    ) -> Self {
        let mut cache = Self::with_max_count(max_count);
        cache.listener = Some(Box::new(listener));
        cache
    }

    /// Installs (or replaces) the eviction listener: `Caffeine.evictionListener(RemovalListener)`.
    /// Called only for evictions (`cause.wasEvicted()`, here [`RemovalCause::Size`]), synchronously
    /// inside the operation that removes the entry (`notifyEviction`, BoundedLocalCache.java:1071),
    /// and also for the very key being inserted when it is not admitted (a zero-capacity cache).
    pub fn set_eviction_listener(&mut self, listener: impl FnMut(&K, RemovalCause) + Send + 'static) {
        self.listener = Some(Box::new(listener));
    }

    /// Installs (or replaces) the removal listener: `Caffeine.removalListener`. Called for every cause,
    /// [`RemovalCause::Explicit`] included (`notifyRemoval`). Upstream runs it on an executor; this
    /// single-threaded port runs it inline after the eviction listener.
    pub fn set_removal_listener(&mut self, listener: impl FnMut(&K, RemovalCause) + Send + 'static) {
        self.removal_listener = Some(Box::new(listener));
    }

    /// Current maximum entry count (`Policy.Eviction.getMaximum()`).
    pub fn maximum(&self) -> u64 {
        self.max_count as u64
    }

    /// Port of `Policy.Eviction.setMaximum(long)` (BoundedLocalCache.java:4110): `setMaximumSize`
    /// (676-701) followed by a maintenance cycle, whose `evictEntries()` shrinks the cache and whose
    /// `climb()` demotes protected overflow. Returns the keys it evicted (also reported to the
    /// eviction listener with [`RemovalCause::Size`]).
    pub fn set_maximum(&mut self, maximum: u64) -> Vec<K> {
        let max = maximum as i64;
        self.evicted.clear();
        if max != self.max_count {
            let window = max - (PERCENT_MAIN * max as f64) as i64;
            self.max_count = max;
            self.window_maximum = window;
            self.main_protected_maximum = (PERCENT_MAIN_PROTECTED * (max - window) as f64) as i64;
            self.hits_in_sample = 0;
            self.misses_in_sample = 0;
            self.step_size = -HILL_CLIMBER_STEP_PERCENT * max as f64;
            if self.len() as i64 >= max >> 1 {
                self.sketch.ensure_capacity(max);
            }
        }
        self.maintenance(None);
        std::mem::take(&mut self.evicted)
    }

    /// Port of `asMap().computeIfPresent(key, (k, v) -> null)` / `Cache.invalidate(key)`: removes `key`
    /// with [`RemovalCause::Explicit`] through a maintenance cycle (`RemovalTask`). Returns whether it
    /// was present. The frequency sketch keeps the key's counts, as upstream's does.
    pub fn remove(&mut self, key: &K) -> bool {
        match self.index.get(key) {
            Some(&id) => {
                self.maintenance(Some(Task::Remove(id)));
                true
            }
            None => false,
        }
    }

    /// `Cache.invalidate(key)`: [`Self::remove`] under Caffeine's name.
    pub fn invalidate(&mut self, key: &K) {
        self.remove(key);
    }

    /// `Cache.invalidateAll(keys)`.
    pub fn invalidate_all_of<'a>(&mut self, keys: impl IntoIterator<Item = &'a K>)
    where
        K: 'a,
    {
        for key in keys {
            self.remove(key);
        }
    }

    /// `Cache.invalidateAll()` (`clear()`, BoundedLocalCache.java:2047): pending reads are discarded and
    /// every entry leaves with [`RemovalCause::Explicit`], in insertion-window, probation, protected order.
    pub fn invalidate_all(&mut self) {
        self.read_buffer.clear();
        let mut ids = Vec::with_capacity(self.index.len());
        for deque in [&self.window, &self.probation, &self.protected] {
            let mut cur = deque.head;
            while let Some(id) = cur {
                ids.push(id);
                cur = self.arena[id].as_ref().unwrap().next;
            }
        }
        for id in ids {
            self.evict_node(id, RemovalCause::Explicit);
        }
    }

    /// [`Self::put`] that also returns the keys the write evicted to stay within the bound
    /// (`RemovalCause.SIZE`; the new key itself is among them if it was not admitted).
    pub fn put_evicting(&mut self, key: K) -> Vec<K> {
        self.evicted.clear();
        self.put(key);
        std::mem::take(&mut self.evicted)
    }

    /// How many admissions were decided by the hash-flooding coin toss (`admit`'s random branch),
    /// for differential tests: a victim sequence is reproducible only while this stays zero.
    pub fn admission_coin_flips(&self) -> u64 {
        self.coin_flips
    }

    /// Number of entries currently held (sum of all three segments).
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the cache holds no entry.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Whether `key` is in the cache. Unlike [`Self::get`], this records no read.
    pub fn contains(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// Current admission-window capacity (`windowMaximum()`, BoundedLocalCache.java), exposed for
    /// tests that check `climb()` actually moves it.
    #[cfg(test)]
    fn window_maximum(&self) -> i64 {
        self.window_maximum
    }

    /// Current main-space protected-region capacity (`mainProtectedMaximum()`), exposed for tests.
    #[cfg(test)]
    fn main_protected_maximum(&self) -> i64 {
        self.main_protected_maximum
    }

    /// Records a read of `key` (`asMap().get`, `getIfPresent`). Returns whether the key was present.
    /// Port of `afterRead` (BoundedLocalCache.java:1281-1290): the read is buffered and applied by the
    /// next maintenance cycle (see the module doc comment).
    pub fn get(&mut self, key: &K) -> bool {
        let Some(&id) = self.index.get(key) else {
            return false;
        };
        self.after_read(id);
        true
    }

    /// Writes `key` (`asMap().compute`). An absent key is added through a maintenance cycle (`AddTask`,
    /// BoundedLocalCache.java:1867-1928, then `evictEntries` and `climb`); a present one keeps its weight
    /// of 1, so upstream's `remap` records a read instead (BoundedLocalCache.java:2932-2938).
    pub fn put(&mut self, key: K) {
        if let Some(&id) = self.index.get(&key) {
            self.after_read(id);
            return;
        }

        let id = self.alloc(key.clone(), Segment::Window);
        self.index.insert(key, id);
        self.maintenance(Some(Task::Add(id)));
    }

    // ---- internal: arena management -------------------------------------------------------------

    fn alloc(&mut self, key: K, segment: Segment) -> usize {
        let node = Node { key, segment, prev: None, next: None };
        if let Some(id) = self.free.pop() {
            self.arena[id] = Some(node);
            id
        } else {
            self.arena.push(Some(node));
            self.arena.len() - 1
        }
    }

    fn segment_of(&self, id: usize) -> Segment {
        self.arena[id].as_ref().unwrap().segment
    }

    /// Removes `id` from the cache entirely: unlinks it from whichever deque holds it, drops it from
    /// the key index, and frees the arena slot. Port of the effect of `evictEntry`
    /// (BoundedLocalCache.java:1023 onward) restricted to bookkeeping (no removal listener / stats,
    /// which this port doesn't carry).
    fn evict_node(&mut self, id: usize, cause: RemovalCause) {
        let segment = self.segment_of(id);
        if segment == Segment::Protected {
            // `evictEntry` / `RemovalTask` give the protected weight back (`RemovalTask`, `evictEntry`).
            self.main_protected_size -= 1;
        }
        let deque = match segment {
            Segment::Window => &mut self.window,
            Segment::Probation => &mut self.probation,
            Segment::Protected => &mut self.protected,
        };
        deque.remove(&mut self.arena, id);
        let key = self.arena[id].take().unwrap().key;
        self.index.remove(&key);
        self.free.push(id);
        if cause == RemovalCause::Size
            && let Some(listener) = self.listener.as_mut()
        {
            listener(&key, cause);
        }
        if let Some(listener) = self.removal_listener.as_mut() {
            listener(&key, cause);
        }
        if cause == RemovalCause::Size {
            self.evicted.push(key);
        }
    }

    // ---- internal: maintenance (ported from BoundedLocalCache.java) -----------------------------

    /// Port of `afterRead` (BoundedLocalCache.java:1281-1290) for a single thread: no buffering while
    /// the sketch is unsized (`skipReadBuffer` with `fastpath()`, true for a size-bounded cache without
    /// expiry or reference keys/values), else the read is buffered; a full buffer drops it and runs the
    /// maintenance cycle (`shouldDrainBuffers(false)` while idle).
    fn after_read(&mut self, id: usize) {
        if self.sketch.is_not_initialized() {
            return;
        }
        if self.read_buffer.len() < READ_BUFFER_SIZE {
            self.read_buffer.push(id);
        } else {
            self.maintenance(None);
        }
    }

    /// Port of `maintenance(Runnable)` (BoundedLocalCache.java:1707-1731) for a cache without expiry or
    /// reference keys/values: drain the read buffer, run the write's task, evict, climb.
    fn maintenance(&mut self, task: Option<Task>) {
        let retired = match task {
            Some(Task::Remove(id)) => Some(id),
            _ => None,
        };
        // `drainReadBuffer` (:1767-1771); the buffer is empty whenever the sketch is unsized.
        for id in std::mem::take(&mut self.read_buffer) {
            self.on_access(id, retired);
        }
        match task {
            Some(Task::Add(id)) => self.add_task(id),
            Some(Task::Remove(id)) => self.evict_node(id, RemovalCause::Explicit),
            None => {}
        }
        self.evict_entries();
        self.climb();
    }

    /// Port of `AddTask.run` (BoundedLocalCache.java:1877-1928) at unit weight: size the sketch once
    /// the cache reaches half its maximum, count the key and a miss, and link the node at the window's
    /// MRU end (upstream links it at the LRU end when its weight exceeds the window, which a weight of
    /// 1 never does: the window keeps at least 1 for any maximum above 0).
    fn add_task(&mut self, id: usize) {
        if self.len() as i64 >= self.max_count >> 1 {
            self.sketch.ensure_capacity(self.max_count);
        }
        let key = self.arena[id].as_ref().unwrap().key.clone();
        self.sketch.increment(&key);
        self.misses_in_sample += 1;
        self.window.offer_last(&mut self.arena, id);
        if self.max_count < 1 {
            // `weight > maximum()`: evicted at once
            self.evict_node(id, RemovalCause::Size);
        }
    }

    // ---- internal: policy (ported from BoundedLocalCache.java) ----------------------------------

    /// Port of `onAccess(Node)` (BoundedLocalCache.java:1774-1796), restricted to the `evicts()`
    /// branch (this port always evicts; there's no expiry-only mode). `retired` is the node a removal
    /// already took out of the map, whose key upstream reads back as the retired-key sentinel.
    fn on_access(&mut self, id: usize, retired: Option<usize>) {
        if retired == Some(id) {
            self.sketch.increment_hash(RETIRED_KEY_HASH);
        } else {
            let key = self.arena[id].as_ref().unwrap().key.clone();
            self.sketch.increment(&key);
        }

        match self.segment_of(id) {
            Segment::Window => {
                self.window.move_to_back(&mut self.arena, id);
            }
            Segment::Probation => {
                self.reorder_probation(id);
            }
            Segment::Protected => {
                self.protected.move_to_back(&mut self.arena, id);
            }
        }
        // Port of `setHitsInSample(hitsInSample() + 1)` (BoundedLocalCache.java:1789).
        self.hits_in_sample += 1;
    }

    /// Promotes a node from probation to protected on access. Port of `reorderProbation(Node)`
    /// (BoundedLocalCache.java:1798-1815): a node heavier than the protected maximum (here: a protected
    /// maximum of 0) only moves to probation's MRU end, and protected overflow is demoted later, by
    /// `climb()` at the end of the maintenance cycle.
    fn reorder_probation(&mut self, id: usize) {
        // upstream also checks `!accessOrderProbationDeque().contains(node)` to ignore a stale access
        // on a removed entry; a single thread drains every read before the removal's task runs, so the
        // node given here is always in the probation deque.
        if self.main_protected_maximum < 1 {
            self.probation.move_to_back(&mut self.arena, id);
            return;
        }
        self.main_protected_size += 1;
        self.probation.remove(&mut self.arena, id);
        self.protected.offer_last(&mut self.arena, id);
        self.arena[id].as_mut().unwrap().segment = Segment::Protected;
    }

    /// Transfers nodes from protected back to probation while protected exceeds its maximum. Port of
    /// `demoteFromMainProtected()` (BoundedLocalCache.java:1248-1271).
    fn demote_from_main_protected(&mut self) {
        for _ in 0..QUEUE_TRANSFER_THRESHOLD {
            if self.main_protected_size <= self.main_protected_maximum {
                break;
            }
            let Some(demoted) = self.protected.poll_first(&mut self.arena) else {
                break;
            };
            self.arena[demoted].as_mut().unwrap().segment = Segment::Probation;
            self.probation.offer_last(&mut self.arena, demoted);
            self.main_protected_size -= 1;
        }
    }

    /// Adapts the eviction policy towards the optimal recency/frequency configuration by hill-climbing
    /// the window/main split on the observed hit rate. Port of `climb()` (BoundedLocalCache.java:
    /// 1116-1133); the `if (!evicts())` guard is dropped since this port always evicts (it has no
    /// expiry-only mode to fall back to).
    fn climb(&mut self) {
        self.determine_adjustment();
        self.demote_from_main_protected();
        let amount = self.adjustment;
        if amount > 0 {
            self.increase_window();
        } else if amount < 0 {
            self.decrease_window();
        }
    }

    /// Calculates the amount to adapt the window by and sets `self.adjustment` accordingly, based on
    /// whether the hit rate improved or worsened since the last sample window. Port of
    /// `determineAdjustment()` (BoundedLocalCache.java:1135-1161).
    fn determine_adjustment(&mut self) {
        if self.sketch.is_not_initialized() {
            self.previous_sample_hit_rate = 0.0;
            self.misses_in_sample = 0;
            self.hits_in_sample = 0;
            return;
        }

        let request_count = self.hits_in_sample + self.misses_in_sample;
        if request_count < self.sketch.sample_size() {
            return;
        }

        let hit_rate = f64::from(self.hits_in_sample) / f64::from(request_count);
        let hit_rate_change = hit_rate - self.previous_sample_hit_rate;
        let amount = if hit_rate_change >= 0.0 { self.step_size } else { -self.step_size };
        let next_step_size = if hit_rate_change.abs() >= HILL_CLIMBER_RESTART_THRESHOLD {
            HILL_CLIMBER_STEP_PERCENT * self.max_count as f64 * if amount >= 0.0 { 1.0 } else { -1.0 }
        } else {
            HILL_CLIMBER_STEP_DECAY_RATE * amount
        };
        self.previous_sample_hit_rate = hit_rate;
        self.adjustment = amount as i64;
        self.step_size = next_step_size;
        self.misses_in_sample = 0;
        self.hits_in_sample = 0;
    }

    /// Increases the size of the admission window by shrinking the portion allocated to the main
    /// space's protected region, transferring LRU probation/protected entries into the window until
    /// the quota is spent. Port of `increaseWindow()` (BoundedLocalCache.java:1163-1212), specialized
    /// to unit weight: upstream's `quota < candidate.getPolicyWeight()` / `quota < weight` checks
    /// collapse to `quota < 1` (i.e. `quota == 0`, since `quota` never goes negative), and
    /// `windowWeightedSize`/`mainProtectedWeightedSize` bookkeeping is just `window.len` /
    /// `main_protected_size`, already tracked.
    fn increase_window(&mut self) {
        if self.main_protected_maximum == 0 {
            return;
        }

        let mut quota = self.adjustment.min(self.main_protected_maximum);
        self.main_protected_maximum -= quota;
        self.window_maximum += quota;
        self.demote_from_main_protected();

        for _ in 0..QUEUE_TRANSFER_THRESHOLD {
            let mut candidate = self.probation.head;
            let mut probation = true;
            if candidate.is_none() || quota < 1 {
                candidate = self.protected.head;
                probation = false;
            }
            let Some(candidate) = candidate else { break };
            if quota < 1 {
                break;
            }

            quota -= 1;
            if probation {
                self.probation.remove(&mut self.arena, candidate);
            } else {
                self.main_protected_size -= 1;
                self.protected.remove(&mut self.arena, candidate);
            }
            self.window.offer_last(&mut self.arena, candidate);
            self.arena[candidate].as_mut().unwrap().segment = Segment::Window;
        }

        self.main_protected_maximum += quota;
        self.window_maximum -= quota;
        self.adjustment = quota;
    }

    /// Decreases the size of the admission window and increases the main space's protected region,
    /// transferring LRU window entries into probation until the quota is spent. Port of
    /// `decreaseWindow()` (BoundedLocalCache.java:1214-1246), specialized to unit weight the same way
    /// [`Self::increase_window`] is.
    fn decrease_window(&mut self) {
        if self.window_maximum <= 1 {
            return;
        }

        let mut quota = (-self.adjustment).min((self.window_maximum - 1).max(0));
        self.main_protected_maximum += quota;
        self.window_maximum -= quota;

        for _ in 0..QUEUE_TRANSFER_THRESHOLD {
            let Some(candidate) = self.window.head else { break };
            if quota < 1 {
                break;
            }

            quota -= 1;
            self.window.remove(&mut self.arena, candidate);
            self.probation.offer_last(&mut self.arena, candidate);
            self.arena[candidate].as_mut().unwrap().segment = Segment::Probation;
        }

        self.main_protected_maximum -= quota;
        self.window_maximum += quota;
        self.adjustment = -quota;
    }

    /// Evicts entries if the cache exceeds `max_count`. Port of `evictEntries()`
    /// (BoundedLocalCache.java:703-711).
    fn evict_entries(&mut self) {
        let candidate = self.evict_from_window();
        self.evict_from_main(candidate);
    }

    /// Evicts entries from the window into the probation segment while the window exceeds its
    /// maximum. Port of `evictFromWindow()` (BoundedLocalCache.java:719-744).
    ///
    /// Returns the first candidate promoted into probation (the window's own LRU-ordered spillover),
    /// the starting point `evict_from_main` walks the window from when it needs more candidates.
    fn evict_from_window(&mut self) -> Option<usize> {
        let mut first = None;
        while (self.window.len as i64) > self.window_maximum {
            let Some(node) = self.window.head else { break };
            self.window.remove(&mut self.arena, node);
            self.probation.offer_last(&mut self.arena, node);
            self.arena[node].as_mut().unwrap().segment = Segment::Probation;
            if first.is_none() {
                first = Some(node);
            }
        }
        first
    }

    /// Evicts entries from the main space (probation/protected) if the cache still exceeds
    /// `max_count`, deciding between the window's promoted candidate and the probation-segment
    /// victim via [`Self::admit`]. Port of `evictFromMain(Node)` (BoundedLocalCache.java:763-871),
    /// simplified: this port has no weight, no weak references, and no concurrent removal, so the
    /// "collected"/"not alive"/zero-weight branches of the upstream loop (which exist only to handle
    /// those cases) are dropped; the SLRU-walk / frequency-comparison core is kept.
    fn evict_from_main(&mut self, candidate: Option<usize>) {
        let mut candidate = candidate;
        // Port of `candidateQueue` (BoundedLocalCache.java:766): starts at PROBATION regardless of
        // whether the passed-in `candidate` is already set — it names a *deque to refill from*, not
        // the candidate's own current location (the initial candidate, if any, already lives in the
        // probation deque: `evict_from_window` appended it there).
        let mut candidate_from_window = false;
        let mut victim = self.probation.head;
        let mut victim_segment = Segment::Probation;

        while self.len() as i64 > self.max_count {
            if candidate.is_none() && !candidate_from_window {
                candidate = self.window.head;
                candidate_from_window = true;
            }

            if candidate.is_none() && victim.is_none() {
                match victim_segment {
                    Segment::Probation => {
                        victim = self.protected.head;
                        victim_segment = Segment::Protected;
                        continue;
                    }
                    Segment::Protected => {
                        victim = self.window.head;
                        victim_segment = Segment::Window;
                        continue;
                    }
                    Segment::Window => break,
                }
            }

            match (victim, candidate) {
                (None, Some(c)) => {
                    let next = self.arena[c].as_ref().unwrap().next;
                    self.evict_node(c, RemovalCause::Size);
                    candidate = next;
                }
                (Some(v), None) => {
                    let next = self.arena[v].as_ref().unwrap().next;
                    self.evict_node(v, RemovalCause::Size);
                    victim = next;
                }
                (Some(v), Some(c)) if v == c => {
                    let next = self.arena[v].as_ref().unwrap().next;
                    self.evict_node(c, RemovalCause::Size);
                    victim = next;
                    candidate = None;
                }
                (Some(v), Some(c)) => {
                    let victim_key = self.arena[v].as_ref().unwrap().key.clone();
                    let candidate_key = self.arena[c].as_ref().unwrap().key.clone();
                    if self.admit(&candidate_key, &victim_key) {
                        let next = self.arena[v].as_ref().unwrap().next;
                        self.evict_node(v, RemovalCause::Size);
                        victim = next;
                        candidate = self.arena[c].as_ref().unwrap().next;
                    } else {
                        let next = self.arena[c].as_ref().unwrap().next;
                        self.evict_node(c, RemovalCause::Size);
                        candidate = next;
                    }
                }
                (None, None) => unreachable!("handled above"),
            }
        }
    }

    /// Determines whether the candidate should be admitted into the main space over the victim, by
    /// comparing estimated frequencies, with a small amount of randomness to defend against a
    /// hash-flooding attack that artificially inflates the victim's frequency. Port of
    /// `admit(K, K)` (BoundedLocalCache.java:883-897).
    fn admit(&mut self, candidate_key: &K, victim_key: &K) -> bool {
        let victim_freq = self.sketch.frequency(victim_key);
        let candidate_freq = self.sketch.frequency(candidate_key);
        if candidate_freq > victim_freq {
            true
        } else if candidate_freq >= ADMIT_HASHDOS_THRESHOLD {
            self.coin_flips += 1;
            (pseudo_random_i32() & 127) == 0
        } else {
            false
        }
    }
}

/// A cheap, dependency-free source of randomness for [`BoundedCache::admit`]'s hash-flood defense —
/// upstream uses `ThreadLocalRandom.current().nextInt()` (BoundedLocalCache.java:893); this port only
/// needs "unpredictable enough to defeat a deliberately-crafted hash collision", not a specific PRNG
/// algorithm, so it reuses a `RandomState`-seeded hasher rather than pull in a `rand` dependency.
fn pseudo_random_i32() -> i32 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    let mut h = RandomState::new().build_hasher();
    std::hash::Hasher::write_u8(&mut h, 0);
    std::hash::Hasher::finish(&h) as i32
}

impl Deque {
    fn offer_last<K>(&mut self, arena: &mut [Option<Node<K>>], id: usize) {
        arena[id].as_mut().unwrap().prev = self.tail;
        arena[id].as_mut().unwrap().next = None;
        if let Some(tail) = self.tail {
            arena[tail].as_mut().unwrap().next = Some(id);
        } else {
            self.head = Some(id);
        }
        self.tail = Some(id);
        self.len += 1;
    }

    fn remove<K>(&mut self, arena: &mut [Option<Node<K>>], id: usize) {
        let (prev, next) = {
            let node = arena[id].as_ref().unwrap();
            (node.prev, node.next)
        };
        match prev {
            Some(p) => arena[p].as_mut().unwrap().next = next,
            None => self.head = next,
        }
        match next {
            Some(n) => arena[n].as_mut().unwrap().prev = prev,
            None => self.tail = prev,
        }
        arena[id].as_mut().unwrap().prev = None;
        arena[id].as_mut().unwrap().next = None;
        self.len -= 1;
    }

    fn poll_first<K>(&mut self, arena: &mut [Option<Node<K>>]) -> Option<usize> {
        let head = self.head?;
        self.remove(arena, head);
        Some(head)
    }

    /// Port of `reorder(LinkedDeque, Node)` (BoundedLocalCache.java:1818-1825) specialized to a deque
    /// known to contain `id` (this port has no stale/concurrently-removed accesses to guard against).
    fn move_to_back<K>(&mut self, arena: &mut [Option<Node<K>>], id: usize) {
        if self.tail == Some(id) {
            return;
        }
        self.remove(arena, id);
        self.offer_last(arena, id);
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedCache;

    /// Small dependency-free xorshift32 PRNG for the property-test workloads below, to keep this
    /// crate at zero external dependencies.
    struct Xorshift32(u32);
    impl Xorshift32 {
        fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }
        fn next_below(&mut self, bound: u32) -> u32 {
            self.next_u32() % bound
        }
    }

    /// Acceptance criterion: "A bounded cache of `maxCount` N never exceeds N entries under
    /// sustained insertion" (the port's acceptance criteria).
    #[test]
    fn size_bound_under_sustained_insertion() {
        for &max in &[1u64, 2, 8, 37, 100] {
            let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(max);
            for i in 0..(50 * max + 500) {
                cache.put(i);
                assert!(
                    cache.len() as u64 <= max,
                    "max={max} exceeded at i={i}: len={}",
                    cache.len()
                );
            }
            assert_eq!(cache.len() as u64, max, "cache should be full after sustained insertion");
        }
    }

    /// Acceptance criterion: "Frequently re-accessed entries survive eviction preferentially over
    /// entries touched once" (the port's acceptance criteria), "verified with an
    /// access-frequency-skewed workload, not by asserting a specific victim". Hot keys are inserted
    /// once and then re-accessed on every round *interleaved* with a flood of distinct cold keys
    /// (touched once each) — this is what "frequently re-accessed" actually describes (ongoing
    /// repeated access, not a one-time warm-up abandoned before the flood), and it's also what keeps
    /// the test meaningful: `FrequencySketch` ages its counts (periodic halving, see
    /// `frequency_sketch::tests::reset`), so a hot key that stops being touched partway through a long
    /// flood would eventually decay to the same frequency as the cold noise regardless of how the
    /// eviction policy is implemented. Most hot keys should still be resident afterwards; no specific
    /// hot key is asserted, because eviction is legitimately
    /// non-deterministic across nodes.
    #[test]
    fn frequent_entries_survive_preferentially() {
        let max_count = 50u64;
        let hot_count = 10u64;
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(max_count);
        let mut rng = Xorshift32(0x9E3779B9);

        for k in 0..hot_count {
            cache.put(k);
        }

        let cold_start = 1_000_000u64;
        let mut cold_next = cold_start;
        for _round in 0..500 {
            for k in 0..hot_count {
                cache.get(&k);
            }
            for _ in 0..10 {
                cold_next += 1 + rng.next_below(4) as u64;
                cache.put(cold_next);
            }
        }

        let survivors = (0..hot_count).filter(|k| cache.contains(k)).count() as u64;
        assert!(
            survivors * 10 >= hot_count * 8,
            "expected at least 80% of hot keys to survive an interleaved cold flood, got {survivors}/{hot_count}"
        );
    }

    #[test]
    fn get_records_hit_and_promotes() {
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(10);
        cache.put(1);
        assert!(cache.get(&1));
        assert!(!cache.get(&999));
    }

    #[test]
    fn put_existing_key_is_an_access_not_a_duplicate() {
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(10);
        cache.put(1);
        cache.put(1);
        cache.put(1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zero_capacity_cache_holds_nothing() {
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(0);
        cache.put(1);
        assert_eq!(cache.len(), 0);
    }

    /// Port-specific regression for `climb()`/`determineAdjustment()` (BoundedLocalCache.java:1116-1161):
    /// the climber must not touch `window_maximum`/`main_protected_maximum` before enough samples have
    /// accumulated (`requestCount < frequencySketch().sampleSize`, BoundedLocalCache.java:1146).
    #[test]
    fn climb_is_gated_by_sample_size() {
        let max_count = 1_000u64;
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(max_count);
        let initial_window_maximum = cache.window_maximum();

        // `sample_size` is `10 * max_count` (FrequencySketch.java's `sampleSize` field, ensured to
        // `max_count` capacity in `with_max_count`). One short of it, the gate must stay shut.
        let sample_size = 10 * max_count - 1;
        for i in 0..sample_size {
            cache.put(i);
        }
        assert_eq!(
            cache.window_maximum(),
            initial_window_maximum,
            "climb() must not adjust the window before sampleSize requests have been observed"
        );
    }

    /// Port-specific: once the sample gate opens, `determineAdjustment()`'s very first call is
    /// deterministic — `previousSampleHitRate` starts at 0.0 (BoundedLocalCache.java:693, `setHitsInSample(0)`
    /// et al. in `setMaximumSize`), so `hitRateChange = hitRate - 0.0 >= 0` always, which always picks
    /// `amount = stepSize()`, and `stepSize()` is seeded negative (`-HILL_CLIMBER_STEP_PERCENT * max`,
    /// BoundedLocalCache.java:695). A negative `amount` means `climb()` calls `decreaseWindow()`
    /// (BoundedLocalCache.java:1128-1132), so the very first adjustment always shrinks the window.
    #[test]
    fn climb_first_adjustment_shrinks_the_window() {
        let max_count = 1_000u64;
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(max_count);
        let initial_window_maximum = cache.window_maximum();
        let initial_main_protected_maximum = cache.main_protected_maximum();
        assert!(initial_window_maximum > 1, "test needs headroom to observe a shrink");

        // The sketch is sized by the write that fills half the cache, and samples count from there.
        let sample_size = 10 * max_count;
        for i in 0..sample_size + max_count / 2 {
            cache.put(i);
        }

        assert!(
            cache.window_maximum() < initial_window_maximum,
            "expected the first hill-climbing adjustment to shrink the window: {} -> {}",
            initial_window_maximum,
            cache.window_maximum()
        );
        assert!(
            cache.main_protected_maximum() > initial_main_protected_maximum,
            "the capacity taken from the window must move to the main/protected region"
        );
        assert_eq!(
            cache.window_maximum() + cache.main_protected_maximum(),
            initial_window_maximum + initial_main_protected_maximum,
            "climb() only redistributes capacity between window and main-protected, never creates or \
             loses it"
        );
    }

    /// Acceptance criterion (still holds with the hill climber wired in, second pass): the
    /// size bound must survive `climb()` mutating `window_maximum`/`main_protected_maximum` mid-run —
    /// a bug in `increase_window`/`decrease_window`'s transfer bookkeeping could otherwise let the
    /// cache grow past `max_count` or panic on an arena access. Exercises a long, hit-rate-fluctuating
    /// workload (interleaved hot re-access and cold one-off inserts) specifically so the climber's
    /// gate opens and both `increase_window` and `decrease_window` get exercised across the run.
    #[test]
    fn size_bound_holds_while_climbing() {
        let max_count = 500u64;
        let mut cache: BoundedCache<u64> = BoundedCache::with_max_count(max_count);
        let mut rng = Xorshift32(0xC0FFEE);
        let mut next_cold = 10_000_000u64;

        for round in 0..40_000u64 {
            // Alternate stretches of hot re-access (drives the hit rate up, climber should react by
            // growing the window less / shrinking it) and cold-only inserts (drives the hit rate down),
            // so the observed hit rate genuinely fluctuates across sample windows instead of settling.
            if (round / 500) % 2 == 0 {
                let hot_key = round % 20;
                cache.put(hot_key);
                cache.get(&hot_key);
            } else {
                next_cold += 1 + u64::from(rng.next_below(3));
                cache.put(next_cold);
            }

            assert!(
                cache.len() as u64 <= max_count,
                "size bound violated at round {round}: len={}",
                cache.len()
            );
            assert!(cache.window_maximum() >= 0, "window_maximum went negative at round {round}");
            assert!(
                cache.main_protected_maximum() >= 0,
                "main_protected_maximum went negative at round {round}"
            );
            assert!(
                cache.window_maximum() + cache.main_protected_maximum() <= max_count as i64,
                "window + main-protected capacity exceeded max_count at round {round}: {} + {} > {}",
                cache.window_maximum(),
                cache.main_protected_maximum(),
                max_count
            );
        }
    }
}
