package com.github.benmanes.caffeine.cache;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Differential vectors for the caffeine-rs eviction policy: the victim
 * sequence of the real, pinned {@code caffeine-3.1.8.jar} under single-threaded access.
 *
 * <p>The cache is built the way Infinispan's {@code BoundedSegmentedDataContainer} builds it
 * ({@code AbstractInternalDataContainer.applyListener}: {@code maximumSize(n)}, an executor that runs
 * the maintenance task on the calling thread, an eviction listener), and driven through the same
 * map operations the container uses: a write is {@code asMap().compute}, a read {@code asMap().get},
 * a removal {@code asMap().computeIfPresent(k, (k, v) -> null)}.
 *
 * <p>Each vector file starts with {@code max <n>}, then one line per operation:
 * {@code <op> <key> [evicted...]}, where op is {@code p} (write), {@code g} (read), {@code r}
 * (removal) or {@code m} (policy {@code setMaximum}, the key being the new maximum), followed by the
 * keys the eviction listener saw during that operation, in order. A final {@code end} line lists the
 * resident keys in ascending order.
 *
 * <p>The workloads come from a fixed xorshift32 PRNG, so the files are reproducible as long as the
 * admission's hash-flooding coin ({@code ThreadLocalRandom}, BoundedLocalCache.java:893) is never
 * consulted; the Rust replay asserts that it never is.
 */
final class EvictionVectorGen {
  private EvictionVectorGen() {}

  private static final class Rng {
    int x;

    Rng(int seed) {
      x = seed;
    }

    int next() {
      x ^= x << 13;
      x ^= x >>> 17;
      x ^= x << 5;
      return x;
    }

    int below(int bound) {
      return Integer.remainderUnsigned(next(), bound);
    }
  }

  private static final class Recorder {
    final Cache<Integer, Integer> cache;
    final BoundedLocalCache<Integer, Integer> bounded;
    final List<Integer> evicted = new ArrayList<>();
    /** Keys read since the read buffer was last drained. */
    final Set<Integer> buffered = new HashSet<>();
    final StringBuilder out = new StringBuilder();

    Recorder(int max) {
      cache = Caffeine.newBuilder()
          .maximumSize(max)
          .executor(Runnable::run)
          .evictionListener((Integer k, Integer v, RemovalCause cause) -> evicted.add(k))
          .build();
      bounded = (BoundedLocalCache<Integer, Integer>) cache.asMap();
      out.append("max ").append(max).append('\n');
    }

    /** A write of a present key is a read ({@code remap} calls {@code afterRead}). */
    void put(int k) {
      long before = bounded.readBuffer.size();
      cache.asMap().compute(k, (key, old) -> key);
      noteBuffered(k, before);
      line('p', k);
    }

    void get(int k) {
      long before = bounded.readBuffer.size();
      cache.asMap().get(k);
      noteBuffered(k, before);
      line('g', k);
    }

    private void noteBuffered(int k, long before) {
      if (bounded.readBuffer.size() > before) {
        buffered.add(k);
      }
    }

    /**
     * Skipped while a read of {@code k} waits in the read buffer: the removal retires the node first
     * and the drain then increments the sketch with the {@code RETIRED_STRONG_KEY} sentinel, whose
     * identity hash code no replay can know.
     */
    void remove(int k) {
      if (buffered.contains(k)) {
        return;
      }
      cache.asMap().computeIfPresent(k, (key, old) -> null);
      line('r', k);
    }

    void setMaximum(int max) {
      cache.policy().eviction().orElseThrow().setMaximum(max);
      line('m', max);
    }

    private void line(char op, int k) {
      out.append(op).append(' ').append(k);
      for (int e : evicted) {
        out.append(' ').append(e);
      }
      out.append('\n');
      evicted.clear();
      if (bounded.readBuffer.size() == 0) {
        buffered.clear();
      }
    }

    void write(Path file) throws IOException {
      out.append("end");
      cache.asMap().keySet().stream().sorted().forEach(k -> out.append(' ').append(k));
      out.append('\n');
      Files.writeString(file, out.toString(), StandardCharsets.US_ASCII);
    }
  }

  public static void main(String[] args) throws IOException {
    Path outDir = Path.of(args.length > 0 ? args[0] : "tests/vectors");
    Files.createDirectories(outDir);

    tiny(outDir.resolve("eviction_tiny.txt"));
    uniform(outDir.resolve("eviction_uniform.txt"));
    readBursts(outDir.resolve("eviction_read_bursts.txt"));
    skewed(outDir.resolve("eviction_skewed.txt"));
    scan(outDir.resolve("eviction_scan.txt"));
    resize(outDir.resolve("eviction_resize.txt"));
  }

  /** Maximum 2 over 16 keys: the lazy sketch is sized by the first write. */
  private static void tiny(Path out) throws IOException {
    Recorder r = new Recorder(2);
    Rng rng = new Rng(0x2545F491);
    for (int i = 0; i < 600; i++) {
      int k = rng.below(16);
      int op = rng.below(10);
      if (op < 5) {
        r.put(k);
      } else if (op < 9) {
        r.get(k);
      } else {
        r.remove(k);
      }
    }
    r.write(out);
  }

  /** Maximum 10 over 40 keys, uniform: writes, reads and removals mixed. */
  private static void uniform(Path out) throws IOException {
    Recorder r = new Recorder(10);
    Rng rng = new Rng(0x9E3779B9);
    for (int i = 0; i < 4000; i++) {
      int k = rng.below(40);
      int op = rng.below(10);
      if (op < 5) {
        r.put(k);
      } else if (op < 9) {
        r.get(k);
      } else {
        r.remove(k);
      }
    }
    r.write(out);
  }

  /**
   * Maximum 20: runs of up to 40 reads between writes, so the 16-slot read buffer fills, drops reads
   * and drains on its own.
   */
  private static void readBursts(Path out) throws IOException {
    Recorder r = new Recorder(20);
    Rng rng = new Rng(0x1234567);
    for (int round = 0; round < 300; round++) {
      int writes = 1 + rng.below(4);
      for (int w = 0; w < writes; w++) {
        r.put(rng.below(100));
      }
      int reads = rng.below(41);
      for (int g = 0; g < reads; g++) {
        r.get(rng.below(100));
      }
    }
    r.write(out);
  }

  /** Maximum 50: a hot set of 15 keys read often among a wide cold range; long enough to climb. */
  private static void skewed(Path out) throws IOException {
    Recorder r = new Recorder(50);
    Rng rng = new Rng(0xC0FFEE);
    for (int i = 0; i < 20000; i++) {
      boolean hot = rng.below(10) < 6;
      int k = hot ? rng.below(15) : 100 + rng.below(3000);
      int op = rng.below(20);
      if (op < 13) {
        r.get(k);
      } else if (op < 19) {
        r.put(k);
      } else {
        r.remove(100 + rng.below(3000));
      }
    }
    r.write(out);
  }

  /**
   * Maximum 100: stretches of one-off inserts (a scan) alternating with stretches that re-read a
   * working set, so the hit rate swings and the hill climber moves the window both ways.
   */
  private static void scan(Path out) throws IOException {
    Recorder r = new Recorder(100);
    Rng rng = new Rng(0xBADC0DE);
    int next = 10_000;
    for (int phase = 0; phase < 30; phase++) {
      if (phase % 2 == 0) {
        for (int i = 0; i < 700; i++) {
          int k = rng.below(80);
          if (rng.below(3) == 0) {
            r.put(k);
          } else {
            r.get(k);
          }
        }
      } else {
        for (int i = 0; i < 700; i++) {
          r.put(next++);
        }
      }
    }
    r.write(out);
  }

  /** Maximum 30, shrunk and grown through the policy between stretches of mixed access. */
  private static void resize(Path out) throws IOException {
    Recorder r = new Recorder(30);
    Rng rng = new Rng(0x5EED);
    int[] maxima = {30, 12, 40, 5, 25, 1, 18};
    for (int max : maxima) {
      r.setMaximum(max);
      for (int i = 0; i < 400; i++) {
        int k = rng.below(400);
        int op = rng.below(10);
        if (op < 5) {
          r.put(k);
        } else if (op < 9) {
          r.get(k);
        } else {
          r.remove(k);
        }
      }
    }
    r.write(out);
  }
}
