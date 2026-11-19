//! An unofficial Rust port of the eviction policy of [Caffeine](https://github.com/ben-manes/caffeine) 3.1.8, the
//! Java caching library: the W-TinyLFU admission window, the probation and protected SLRU segments, the
//! [`FrequencySketch`] and the hill climber that sizes the window.
//!
//! It decides which keys a bounded cache keeps, and nothing else: a [`BoundedCache`] holds keys, not values, so the
//! caller stores the values in a map of its own and drops the ones whose keys are evicted. It is synchronous and
//! single-threaded (the caller synchronises), and has no loading, reference-based eviction, expiry or statistics.
//! For the same sequence of operations it evicts the same keys as Caffeine running its maintenance inline, which the
//! tests check against output recorded from the real `caffeine-3.1.8.jar`.
//!
//! Keys hash with [`JavaHashCode`], Java's `hashCode()` for the boxed number types, so the frequency sketch sees the
//! hashes Caffeine sees.
//!
//! # Example
//!
//! ```
//! use caffeine::BoundedCache;
//!
//! let mut cache = BoundedCache::<i32>::with_max_count(100);
//! for key in 0..100 {
//!     cache.put(key);
//! }
//! // Key 7 is read often, so it is frequent enough to stay.
//! for _ in 0..10 {
//!     assert!(cache.get(&7));
//! }
//! // The cache is full: every new key evicts one.
//! let evicted = cache.put_evicting(1000);
//! assert_eq!(evicted.len(), 1);
//! assert_eq!(cache.len(), 100);
//! assert!(cache.contains(&7));
//! ```
//!
//! # Modules
//!
//! - [`eviction`]: [`BoundedCache`], the W-TinyLFU policy, with its eviction and removal listeners.
//! - [`frequency_sketch`]: [`FrequencySketch`], the 4-bit count-min sketch the admission decision reads.

#![warn(missing_docs)]

pub mod eviction;
pub mod frequency_sketch;

pub use eviction::{BoundedCache, EvictionListener, RemovalCause};
pub use frequency_sketch::{FrequencySketch, JavaHashCode};
