# caffeine-rs

<!-- languages:begin -->
[English](README.md) · **Русский** · [中文](README.zh-CN.md) · [Español](README.es.md)
<!-- languages:end -->
<!-- translation of README.md: 7a907a67ba71 -->

Неофициальный порт на Rust политики вытеснения из [Caffeine](https://github.com/ben-manes/caffeine) 3.1.8, библиотеки
кэширования для Java: окно допуска W-TinyLFU, сегменты SLRU probation и protected, частотный скетч и hill climber,
который подбирает размер окна. Проект не связан с проектом Caffeine и не одобрен им; Caffeine распространяется по
лицензии Apache License 2.0 ([NOTICE](NOTICE)).

Он решает, какие ключи оставляет ограниченный кэш, и ничего больше: кэш хранит ключи, а значения вызывающий код
держит в собственной таблице. Он синхронный и однопоточный, в нём нет загрузки, вытеснения по ссылкам, таймеров
истечения срока и статистики. При тех же операциях он вытесняет те же ключи, что и Caffeine: последовательности
вытесненных ключей и частотный скетч сверяются с выводом, записанным с настоящего `caffeine-3.1.8.jar`
(`tests/vectors/`, созданы программами из `upstream-tools/`).

Он написан для [mock-server](https://github.com/oxidestack/mock-server) — MockServer на Rust, который должен вести себя
так же, как Java-оригинал, — чтобы его кэши Infinispan вытесняли те же записи, что и Caffeine.

Статус: до первого выпуска. Крейт ещё не опубликован на crates.io; первым выпуском станет `0.1.0`.

## Установка

Ещё не опубликован. Когда `0.1.0` появится на crates.io, зависимость будет такой; пакет называется `caffeine-rs`, а в
коде — `caffeine`:

```toml
[dependencies]
caffeine-rs = "0.1"
```

До тех пор зависьте от репозитория: `caffeine-rs = { git = "https://github.com/oxidestack/caffeine-rs" }`. Крейту
нужен Rust 1.88 или новее, зависимостей у него нет.

## Пример

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

Тип ключа реализует `JavaHashCode` — Java-метод `hashCode()` для того же значения, — чтобы частотный скетч видел те же
хеши, что и Caffeine; для `i32`, `u32`, `i64` и `u64` реализация уже есть.

## Документация

- Руководства на английском, русском, китайском и испанском: <https://oxidestack.github.io/caffeine-rs/>, включая
  [обновление](docs/i18n/ru/guide/upgrading.md) между выпусками.
- Документация API: на docs.rs, когда крейт будет опубликован; до тех пор — `cargo doc --open`.
- Что изменилось в каждом выпуске: [CHANGELOG.md](CHANGELOG.md).

## Сборка и тесты

```sh
git submodule update --init   # optional: the pinned Caffeine sources, for reference only
cargo test                    # unit tests and the recorded vectors
make lint                     # clippy, warnings are errors
make deny                     # licences, advisories and sources (cargo install cargo-deny --locked)
make public-check             # the publication gate
make site                     # the documentation site
```

Для пересоздания векторов нужен JDK: см. `upstream-tools/generate.sh`. Как внести вклад, описано в
[CONTRIBUTING.md](CONTRIBUTING.md); коммиты подписываются (DCO).

## Помощь

Вопрос о том, как работает крейт, можно сначала задать [DeepWiki](https://deepwiki.com/oxidestack/caffeine-rs): он
отвечает по коду и документации этого репозитория, а где он расходится с документацией, права документация. Вопросы,
которые он не решил, — в [Discussions](https://github.com/oxidestack/caffeine-rs/discussions), ошибки — в
[issues](https://github.com/oxidestack/caffeine-rs/issues).

## Лицензия

Код: Apache-2.0 ([LICENSE](LICENSE)). Документация: CC-BY-4.0 ([LICENSE-docs](LICENSE-docs)). Что портирует крейт,
указано в [NOTICE](NOTICE).
