package com.github.benmanes.caffeine.cache;

import java.io.IOException;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Reference-vector generator for the caffeine-rs port of {@link FrequencySketch}. Placed in the same package as the real, pinned {@code FrequencySketch} class
 * (loaded from the published {@code caffeine-3.1.8.jar}, not recompiled from source) so it can call
 * its package-private constructor, fields and methods directly — no reflection.
 *
 * <p>Run via {@code upstream-tools/generate.sh}, which fetches the pinned jar, compiles this file
 * against it, and writes the vectors this class prints into {@code tests/vectors/}.
 */
final class VectorGen {
  private VectorGen() {}

  public static void main(String[] args) throws IOException {
    Path outDir = Path.of(args.length > 0 ? args[0] : "tests/vectors");
    Files.createDirectories(outDir);

    writeSpreadRehash(outDir.resolve("frequency_sketch_spread_rehash.csv"));
    writeBasicFrequencies(outDir.resolve("frequency_sketch_basic.csv"));
    writeFullTable(outDir);
  }

  /** Direct outputs of the two package-private hash functions, for representative int inputs. */
  private static void writeSpreadRehash(Path out) throws IOException {
    StringBuilder sb = new StringBuilder("input,spread,rehash_of_spread\n");
    int[] inputs = {
      0, 1, -1, 2, -2, 7, 42, -42, 100, -100, 1000, -1000, 12345, -12345,
      Integer.MAX_VALUE, Integer.MIN_VALUE, Integer.MAX_VALUE - 1, Integer.MIN_VALUE + 1,
      0x7fffffff, 0x80000000, 0xdeadbeef, 0x12345678, -305419896, 305419896,
    };
    for (int x : inputs) {
      int spread = FrequencySketch.spread(x);
      int rehash = FrequencySketch.rehash(spread);
      sb.append(x).append(',').append(spread).append(',').append(rehash).append('\n');
    }
    Files.writeString(out, sb.toString(), StandardCharsets.US_ASCII);
  }

  /**
   * A deterministic, non-saturating increment workload (capacity 64, keys 0..299, key {@code k}
   * incremented {@code k % 5} times, boxed {@code Integer} keys so {@code hashCode() == k}) and the
   * resulting {@link FrequencySketch#frequency}. Small enough to stay well clear of the sketch's
   * {@code sampleSize} (no reset fires), so this vector exercises {@code increment}/{@code frequency}
   * without the aging path.
   */
  private static void writeBasicFrequencies(Path out) throws IOException {
    FrequencySketch<Integer> sketch = new FrequencySketch<>();
    sketch.ensureCapacity(64);

    int keyCount = 300;
    for (int k = 0; k < keyCount; k++) {
      int times = k % 5;
      for (int t = 0; t < times; t++) {
        sketch.increment(k);
      }
    }

    StringBuilder sb = new StringBuilder("key,frequency\n");
    for (int k = 0; k < keyCount; k++) {
      sb.append(k).append(',').append(sketch.frequency(k)).append('\n');
    }
    Files.writeString(out, sb.toString(), StandardCharsets.US_ASCII);
  }

  /**
   * Port target for the upstream {@code FrequencySketchTest.full()} test (upstream test source,
   * FrequencySketchTest.java:120-135): capacity 512, {@code sampleSize} forced to
   * {@code Integer.MAX_VALUE} (package-private field, set directly) so the 100,000-key increment
   * loop never triggers a reset, then every table slot is dumped before and after an explicit
   * {@code reset()}. Upstream's own assertions are that every slot has exactly 64 set bits before
   * reset and equals {@code RESET_MASK} after — this vector pins the exact table contents, a
   * stronger, bit-exact version of that same check.
   */
  private static void writeFullTable(Path outDir) throws IOException {
    FrequencySketch<Integer> sketch = new FrequencySketch<>();
    sketch.ensureCapacity(512);
    sketch.sampleSize = Integer.MAX_VALUE;
    for (int i = 0; i < 100_000; i++) {
      sketch.increment(i);
    }
    dumpTable(sketch.table, outDir.resolve("frequency_sketch_full_before_reset.hex"));

    sketch.reset();
    dumpTable(sketch.table, outDir.resolve("frequency_sketch_full_after_reset.hex"));
  }

  private static void dumpTable(long[] table, Path out) throws IOException {
    StringBuilder sb = new StringBuilder();
    for (long value : table) {
      sb.append(String.format("%016x%n", value));
    }
    Files.writeString(out, sb.toString(), StandardCharsets.US_ASCII);
  }
}
