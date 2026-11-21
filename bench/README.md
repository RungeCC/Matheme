# Matheme Benchmark & Performance Regression Suite

Built on [`criterion`](https://docs.rs/criterion) (`0.5`), standard `cargo test` regression guards, and a Wolfram Language end-to-end benchmark script.

## Layout

| Path | Role |
|---|---|
| `bench/Cargo.toml` | `matheme-bench` workspace crate using `criterion` (with HTML reports). |
| `bench/src/lib.rs` | Shared Typst document workloads (cubic curves, simple polygons, overlapping stars, dense self-intersecting polygons, stroked shapes, 5k/9k glyph cache churn, 4-page academic doc, 6-page `Evaluate` doc). |
| `bench/benches/graphics_perf.rs` | Criterion benchmarks for `matheme-graphics`: `winding`, `stroke_bbox`, and `glyph_cache` (PF-01, PF-02, PF-04, PF-05). |
| `bench/benches/pipeline_perf.rs` | Criterion benchmarks for `matheme-world` compile+`Evaluate`, `matheme-wstp` symbol resolution, and 4-page E2E pipeline (PF-03, PF-05). |
| `bench/benches/world_perf.rs` | Criterion benchmarks comparing per-call `MathemeWorld` creation vs persistent `ReusableWorld` (migrated from `matheme-world/examples/bench.rs`). |
| `bench/tests/perf_regressions.rs` | Automated `cargo test` complexity/behavior regression guards (winding vertex/edge budget degradation, LRU cache hit-rate & generation stability, multi-page `Pages` consistency, WSTP symbol resolution). |
| `bench/wolfram_bench.wl` | End-to-end Wolfram Language benchmark comparing native `"Graphics"` vs `"SVG"` vs `"PDF"` imports. |

## Usage

### Run all Criterion benchmarks

```powershell
cargo bench -p matheme-bench
```

Save and compare baselines across commits:

```powershell
cargo bench -p matheme-bench -- --save-baseline main
# ... make optimizations ...
cargo bench -p matheme-bench -- --baseline main
```

Run a single benchmark target or filter:

```powershell
cargo bench -p matheme-bench --bench graphics_perf -- winding
cargo bench -p matheme-bench --bench pipeline_perf -- wstp_symbol_resolve
```

HTML reports and plots are written to `target/criterion/report/index.html`.

### Run performance & correctness regression tests

```powershell
cargo test -p matheme-bench
```

### Run Wolfram Language end-to-end benchmark

After building the paclet (`nu scripts/build.nu`):

```powershell
wolframscript -file bench/wolfram_bench.wl
```

## Measured Results (Windows x86-64, Release Profile)

Comparison between pre-fix baseline (`a8c4c24`) and post-fix Criterion measurements:

| Benchmark | Audit Target | Pre-fix | Post-fix (Criterion) | Notes |
|---|---|---:|---:|---|
| `glyph_cache/churn_5k_unique_keys_extract_and_wl` | PF-02, PF-05 | 292.48 ms | **13.85 ms** | **21.1× speedup** — O(1) LRU (8192 cap) eliminates 4096-entry `clear()` cliff; direct affine transform + static WL segment constants |
| `glyph_cache/churn_9k_over_capacity_lru_steady` | PF-02, PF-05 | — | **302.19 ms** | −28.5% vs pass 1 (422.69 ms) after removing `BezPath` round-trip & hoisting constant WL `Expr`s |
| `winding/fast_path_500_simple_polys` | PF-01, PF-04 | 0.31 ms | **143.6 µs** | **2.16× speedup** — O(1) zero-alloc triangle/quad fast path + zero-copy `WindingResolution::Unchanged` |
| `winding/slow_path_16x19_stars` | PF-01 | 1.29 ms | **1.08 ms** | **16% faster** — sweep-line x-sorted single-pass mutual edge splitting + flatten reuse |
| `winding/dense_self_intersecting/250` | PF-01 | 0.84 ms | **725.1 µs** | **14% faster** — O(E log E + K) sweep-line pruning |
| `winding/dense_self_intersecting/1500` | PF-01, IE-07 | unbounded O(E²) | **73.6 µs** | Bounded by `MAX_WINDING_RESOLVE_EDGES` budget; emits `DegradationWarning::NonZeroWindingApproximated` |
| `stroke_bbox/600_lines_rects_circles` | PF-04, IE-09 | 0.93 ms | **595–662 µs** | **~1.5× speedup** — exact analytical O(1) bbox for lines (all caps) & axis-aligned rects + curve bbox cache |
| `compile_evaluate/svg_6p_with_25_evaluate_exprs` | PF-03 | 5.56 ms | **6.08 ms** | Identical to `svg_6p_no_evaluate` (6.11 ms): ~0% overhead with comemo-cached eval & moved final `Scope` |
| `wstp_symbol_resolve/160k_mixed_system_and_global` | PF-05, AP-04 | 20.50 ms | **18.48 ms** | Replaced arbitrary 120-symbol whitelist with exact kernel-side wire context isolation in `KernelSession` + direct `` System` `` prefixing |
