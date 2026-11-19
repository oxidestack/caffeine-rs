# caffeine-rs

<!-- languages:begin -->
[English](README.md) · [Русский](README.ru.md) · **中文** · [Español](README.es.md)
<!-- languages:end -->
<!-- translation of README.md: 7a907a67ba71 -->

Java 缓存库 [Caffeine](https://github.com/ben-manes/caffeine) 3.1.8 淘汰策略的非官方 Rust 移植：W-TinyLFU 准入窗口、
probation 和 protected 两个 SLRU 段、频率草图（frequency sketch），以及调整窗口大小的 hill climber。本项目与 Caffeine
项目没有关联，也未获得其认可；Caffeine 采用 Apache License 2.0 许可（[NOTICE](NOTICE)）。

它只决定有界缓存保留哪些键，别的什么都不做：缓存保存键，值由调用方保存在自己的映射表中。它是同步、单线程的，没有加载、基于引用的淘汰、
过期定时器或统计功能。对于相同的操作，它淘汰与 Caffeine 相同的键：被淘汰键的序列和频率草图都与从真实的 `caffeine-3.1.8.jar`
记录的输出进行比对（`tests/vectors/`，由 `upstream-tools/` 中的程序生成）。

它是为 [mock-server](https://github.com/oxidestack/mock-server) 编写的。mock-server 是一个必须与 Java 原版行为一致的 Rust
版 MockServer，借助本 crate，它的 Infinispan 缓存淘汰的条目与 Caffeine 相同。

状态：尚未发布。该 crate 还没有在 crates.io 上发布；首个版本将是 `0.1.0`。

## 安装

尚未发布。`0.1.0` 发布到 crates.io 之后，依赖将如下所示；包名为 `caffeine-rs`，在代码中使用 `caffeine`：

```toml
[dependencies]
caffeine-rs = "0.1"
```

在此之前，请直接依赖仓库：`caffeine-rs = { git = "https://github.com/oxidestack/caffeine-rs" }`。该 crate 需要 Rust
1.88 或更高版本，没有任何依赖。

## 示例

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

键类型需要实现 `JavaHashCode`，即 Java 对同一值的 `hashCode()`，这样频率草图看到的哈希值与 Caffeine 相同；`i32`、`u32`、
`i64` 和 `u64` 已经实现。

## 文档

- 指南（英文、俄文、中文和西班牙文）：<https://oxidestack.github.io/caffeine-rs/>，包括版本之间的
  [升级](docs/i18n/zh/guide/upgrading.md)。
- API 文档：crate 发布后位于 docs.rs；在此之前请使用 `cargo doc --open`。
- 每个版本的变更：[CHANGELOG.md](CHANGELOG.md)。

## 构建与测试

```sh
git submodule update --init   # optional: the pinned Caffeine sources, for reference only
cargo test                    # unit tests and the recorded vectors
make lint                     # clippy, warnings are errors
make deny                     # licences, advisories and sources (cargo install cargo-deny --locked)
make public-check             # the publication gate
make site                     # the documentation site
```

重新生成测试向量需要 JDK：见 `upstream-tools/generate.sh`。如何参与贡献见 [CONTRIBUTING.md](CONTRIBUTING.md)；提交需要签署（DCO）。

## 获取帮助

关于本 crate 如何工作的问题，可以先问 [DeepWiki](https://deepwiki.com/oxidestack/caffeine-rs)：它根据本仓库的代码和文档作答，
当它与文档不一致时，以文档为准。它解决不了的问题请到 [Discussions](https://github.com/oxidestack/caffeine-rs/discussions)，
缺陷请提交到 [issues](https://github.com/oxidestack/caffeine-rs/issues)。

## 许可证

代码：Apache-2.0（[LICENSE](LICENSE)）。文档：CC-BY-4.0（[LICENSE-docs](LICENSE-docs)）。本 crate 移植了哪些内容，见
[NOTICE](NOTICE)。
