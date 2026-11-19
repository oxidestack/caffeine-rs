#!/usr/bin/env bash
# Regenerates tests/vectors/frequency_sketch_*.{csv,hex} and tests/vectors/eviction_*.txt from the actual, pinned Caffeine v3.1.8 jar
# (not from a Rust or a recompiled-from-source approximation). Fetches the published jar from Maven
# Central if not already cached under lib/, verifies its sha1 against the one Maven Central publishes,
# compiles upstream-tools/src/.../VectorGen.java (same package as FrequencySketch, so it can reach its
# package-private constructor/fields/methods with no reflection) and EvictionVectorGen.java (the victim
# sequence of a real cache built as Infinispan builds it) against that jar, and runs both.
#
# Usage: upstream-tools/generate.sh
# Verify reproducibility: upstream-tools/generate.sh && git -C .. diff --stat -- tests/vectors  (or cmp)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
JAR="$HERE/lib/caffeine-3.1.8.jar"
JAR_URL="https://repo1.maven.org/maven2/com/github/ben-manes/caffeine/caffeine/3.1.8/caffeine-3.1.8.jar"
JAR_SHA1="24795585df8afaf70a2cd534786904ea5889c047"
OUT="$HERE/out"
VECTORS_DIR="$REPO_ROOT/tests/vectors"

mkdir -p "$HERE/lib" "$OUT"

if [[ ! -f "$JAR" ]]; then
  echo "Fetching $JAR_URL" >&2
  curl -sS -m 30 -o "$JAR" "$JAR_URL"
fi

echo "$JAR_SHA1  $JAR" | shasum -a 1 -c -

SRC="$HERE/src/com/github/benmanes/caffeine/cache"
javac -nowarn -cp "$JAR" -d "$OUT" "$SRC/VectorGen.java" "$SRC/EvictionVectorGen.java"
java -cp "$OUT:$JAR" com.github.benmanes.caffeine.cache.VectorGen "$VECTORS_DIR"
java -cp "$OUT:$JAR" com.github.benmanes.caffeine.cache.EvictionVectorGen "$VECTORS_DIR"

echo "Wrote vectors to $VECTORS_DIR" >&2
