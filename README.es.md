# caffeine-rs

<!-- languages:begin -->
[English](README.md) · [Русский](README.ru.md) · [中文](README.zh-CN.md) · **Español**
<!-- languages:end -->
<!-- translation of README.md: 7a907a67ba71 -->

Un port no oficial a Rust de la política de desalojo de [Caffeine](https://github.com/ben-manes/caffeine) 3.1.8, la
biblioteca de caché para Java: la ventana de admisión W-TinyLFU, los segmentos SLRU probation y protected, el sketch de
frecuencias y el hill climber que dimensiona la ventana. No está afiliado al proyecto Caffeine ni cuenta con su
respaldo; Caffeine se distribuye bajo la Apache License 2.0 ([NOTICE](NOTICE)).

Decide qué claves conserva una caché acotada, y nada más: la caché guarda claves, y quien la usa guarda los valores en
un mapa propio. Es síncrona y de un solo hilo, y no tiene carga, desalojo por referencias, temporizadores de expiración
ni estadísticas. Para las mismas operaciones desaloja las mismas claves que Caffeine: las secuencias de víctimas y el
sketch de frecuencias se comparan con la salida grabada del `caffeine-3.1.8.jar` real (`tests/vectors/`, generada por
los programas de `upstream-tools/`).

Se escribió para [mock-server](https://github.com/oxidestack/mock-server), un MockServer en Rust que debe comportarse
como el original en Java, para que sus cachés de Infinispan desalojen las mismas entradas que Caffeine.

Estado: previo a la primera versión. El crate todavía no está publicado en crates.io; su primera versión será `0.1.0`.

## Instalación

Todavía no publicado. Cuando `0.1.0` esté en crates.io, la dependencia será la siguiente; el paquete es `caffeine-rs`
y el código lo nombra `caffeine`:

```toml
[dependencies]
caffeine-rs = "0.1"
```

Hasta entonces, dependa del repositorio: `caffeine-rs = { git = "https://github.com/oxidestack/caffeine-rs" }`. El
crate necesita Rust 1.88 o posterior y no tiene dependencias.

## Ejemplo

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

Un tipo de clave implementa `JavaHashCode`, el `hashCode()` de Java para el mismo valor, para que el sketch de
frecuencias vea los mismos hashes que Caffeine; `i32`, `u32`, `i64` y `u64` ya lo traen.

## Documentación

- Las guías, en inglés, ruso, chino y español: <https://oxidestack.github.io/caffeine-rs/>, con la
  [actualización](docs/i18n/es/guide/upgrading.md) entre versiones.
- La documentación de la API: en docs.rs cuando el crate se publique; hasta entonces, `cargo doc --open`.
- Qué cambió en cada versión: [CHANGELOG.md](CHANGELOG.md).

## Compilar y probar

```sh
git submodule update --init   # optional: the pinned Caffeine sources, for reference only
cargo test                    # unit tests and the recorded vectors
make lint                     # clippy, warnings are errors
make deny                     # licences, advisories and sources (cargo install cargo-deny --locked)
make public-check             # the publication gate
make site                     # the documentation site
```

Regenerar los vectores requiere un JDK: véase `upstream-tools/generate.sh`. [CONTRIBUTING.md](CONTRIBUTING.md) explica
cómo contribuir; los commits llevan sign-off (DCO).

## Ayuda

Una pregunta sobre cómo funciona el crate puede ir primero a [DeepWiki](https://deepwiki.com/oxidestack/caffeine-rs):
responde a partir del código y la documentación de este repositorio, y donde no coincide con la documentación, la
documentación tiene razón. Las preguntas que no resuelva van a
[Discussions](https://github.com/oxidestack/caffeine-rs/discussions), y los errores a
[issues](https://github.com/oxidestack/caffeine-rs/issues).

## Licencia

Código: Apache-2.0 ([LICENSE](LICENSE)). Documentación: CC-BY-4.0 ([LICENSE-docs](LICENSE-docs)). Lo que porta el
crate figura en [NOTICE](NOTICE).
