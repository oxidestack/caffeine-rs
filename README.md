# caffeine-rs

<!-- languages:begin -->
**English** · [Русский](README.ru.md) · [中文](README.zh-CN.md) · [Español](README.es.md)
<!-- languages:end -->

An unofficial Rust port of the eviction policy of [Caffeine](https://github.com/ben-manes/caffeine) 3.1.8, the Java
caching library: the W-TinyLFU admission window, the probation and protected SLRU segments, the frequency sketch and
the hill climber that sizes the window. It is not affiliated with or endorsed by the Caffeine project; Caffeine is
licensed under the Apache License 2.0 ([NOTICE](NOTICE)).

It decides which keys a bounded cache keeps, and nothing else: the cache holds keys, and the caller keeps the values
in a map of its own. It is synchronous and single-threaded, and has no loading, reference-based eviction, expiry
timers or statistics. For the same operations it evicts the same keys as Caffeine does: the victim sequences and the
frequency sketch are checked against output recorded from the real `caffeine-3.1.8.jar` (`tests/vectors/`, produced
by the programs in `upstream-tools/`).

It was written for [mock-server](https://github.com/oxidestack/mock-server), a Rust MockServer that must behave as the
Java original does, so that its Infinispan caches evict the same entries as Caffeine does.

Status: pre-release. The crate is not published on crates.io yet; its first release will be `0.1.0`.

## Install

Not published yet. Once `0.1.0` is on crates.io, the dependency will be the following; the package is `caffeine-rs`
and the code names it `caffeine`:

```toml
[dependencies]
caffeine-rs = "0.1"
```

Until then, depend on the repository: `caffeine-rs = { git = "https://github.com/oxidestack/caffeine-rs" }`. The crate
needs Rust 1.88 or later and has no dependencies.

## Example

```rust
use caffeine::BoundedCache;

let mut cache = BoundedCache::<i32>::with_max_count(100);
for key in 0..100 {
    cache.put(key);
}
// Key 7 is read often, so it is frequent enough to stay.
for _ in 0..10 {
    cache.get(&7);
}
// The cache is full: every new key evicts one, reported here and to an eviction listener if one is set.
let evicted = cache.put_evicting(1000);
assert_eq!(evicted.len(), 1);
assert!(cache.contains(&7));
```

A key type implements `JavaHashCode`, Java's `hashCode()` for the same value, so that the frequency sketch sees the
hashes Caffeine sees; `i32`, `u32`, `i64` and `u64` come with it.

## Documentation

- The guides, in English, Russian, Chinese and Spanish: <https://oxidestack.github.io/caffeine-rs/>, with
  [upgrading](docs/guide/upgrading.md) between releases.
- The API documentation: on docs.rs once the crate is published; until then, `cargo doc --open`.
- What changed in each release: [CHANGELOG.md](CHANGELOG.md).

## Build and test

```sh
git submodule update --init   # optional: the pinned Caffeine sources, for reference only
cargo test                    # unit tests and the recorded vectors
make lint                     # clippy, warnings are errors
make deny                     # licences, advisories and sources (cargo install cargo-deny --locked)
make public-check             # the publication gate
make site                     # the documentation site
```

Regenerating the vectors needs a JDK: see `upstream-tools/generate.sh`. [CONTRIBUTING.md](CONTRIBUTING.md) says how to
contribute; commits are signed off (DCO).

## Getting help

A question about how the crate works can go to [DeepWiki](https://deepwiki.com/oxidestack/caffeine-rs) first: it
answers from this repository's code and documentation, and where it disagrees with the documentation, the
documentation is right. Questions it does not settle go to
[Discussions](https://github.com/oxidestack/caffeine-rs/discussions), bugs to
[issues](https://github.com/oxidestack/caffeine-rs/issues).

## Licence

Code: Apache-2.0 ([LICENSE](LICENSE)). Documentation: CC-BY-4.0 ([LICENSE-docs](LICENSE-docs)). What the crate ports
is in [NOTICE](NOTICE).
