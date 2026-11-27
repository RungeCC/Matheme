# Matheme Benchmark & Performance Regression Suite

Built on [`criterion`](https://docs.rs/criterion) (`0.5`), standard `cargo test` regression guards, and three Wolfram Language benchmark scripts under `bench/wolfram/`.

## Layout

### Rust (`cargo bench`)

| Path | Role |
|---|---|
| `bench/Cargo.toml` | `matheme-bench` workspace crate using `criterion` (with HTML reports). |
| `bench/src/lib.rs` | Shared Typst document workloads (cubic curves, simple polygons, overlapping stars, dense self-intersecting polygons, stroked shapes, 5k/9k glyph cache churn, 4-page academic doc, 6-page `Evaluate` doc). |
| `bench/benches/boxes_perf.rs` | Criterion benchmarks for `matheme-box::boxes_to_typst`: deep fraction chains, wide rows, and many-row documents (parallel vs sequential parsing). |
| `bench/benches/graphics_perf.rs` | Criterion benchmarks for `matheme-graphics`: `winding`, `stroke_bbox`, and `glyph_cache` (PF-01, PF-02, PF-04, PF-05). |
| `bench/benches/pipeline_perf.rs` | Criterion benchmarks for `matheme-world` compile+`Evaluate`, `matheme-wstp` symbol resolution, and 4-page E2E pipeline (PF-03, PF-05). |
| `bench/benches/world_perf.rs` | Criterion benchmarks comparing per-call `MathemeWorld` creation vs persistent `ReusableWorld` (migrated from `matheme-world/examples/bench.rs`). |
| `bench/tests/perf_regressions.rs` | Automated `cargo test` complexity/behavior regression guards (winding vertex/edge budget degradation, LRU cache hit-rate & generation stability, multi-page `Pages` consistency, WSTP symbol resolution). |

### Wolfram Language (`bench/wolfram/`, run with `wolframscript`)

| Path | Role |
|---|---|
| `bench/wolfram/endtoend_bench.wl` | End-to-end benchmark of the assembled paclet: native `"Graphics"` vs `"SVG"` vs `"PDF"` imports. |
| `bench/wolfram/typstform_bench.wl` | Convert-level benchmark of the `TypstForm` layer: `ToString[TeXForm[…]]` vs the Wolfram-only reference converter vs native `MathemeToTypst`, over flat sums, balanced trees, and `RandomExpression` special-function trees. Also checks that native output matches the reference and exits non-zero when it does not. |
| `bench/wolfram/random_expression.wl` | `RandomExpression` corpus generator (special functions, orthogonal polynomials, hypergeometric, elliptic, …). Loaded by `typstform_bench.wl`, usable standalone. |

`RandomExpression` builds its tree with `Hold[h][args]` as the application head,
so nothing is evaluated while it is assembled and **nothing in the corpus is
`Inactive`** — the default output is `HoldForm[…]` with real heads, i.e. the
shape `TeXForm` and `MathemeToTypst` actually receive. `"OutputForm" ->
"Inactive"` is opt-in and only used by the benchmark's path-coverage rows.

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
cargo bench -p matheme-bench --bench boxes_perf
cargo bench -p matheme-bench --bench boxes_perf -- wide_row
cargo bench -p matheme-bench --bench graphics_perf -- winding
cargo bench -p matheme-bench --bench pipeline_perf -- wstp_symbol_resolve
```

`boxes_perf` also records the sequential baseline of the same parser; set
`RAYON_NUM_THREADS=1` to compare against it.

HTML reports and plots are written to `target/criterion/report/index.html`.

### Run performance & correctness regression tests

```powershell
cargo test -p matheme-bench
```

### Run the Wolfram Language benchmarks

End-to-end (needs the assembled paclet, i.e. `nu scripts/build.nu`):

```powershell
wolframscript -file bench/wolfram/endtoend_bench.wl
```

Convert-level (needs `cargo build --release -p matheme`; loads `Kernel/` in tree
on purpose, so a stale paclet cannot be measured):

```powershell
wolframscript -file bench/wolfram/typstform_bench.wl
```

Both scripts locate the repository from `$InputFileName`, so they can be started
from any working directory. They print a fixed-width table that can be diffed
between commits.

## Measured Results (Windows x86-64, Release Profile)

### Graphics / compile pipeline

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

### TypstForm box → Typst (`bench/wolfram/typstform_bench.wl`)

Median of three `RepeatedTiming` rounds (0.25 s budget each). `TeX` =
`ToString[TeXForm[…]]`, `Ref` = Wolfram-only converter on the same boxes,
`Native` = LibraryLink `boxesToTypst`:

| Case | Form | Leaf | TeX | Ref | Native | TeX/Native | Ref/Native |
|---|---|---:|---:|---:|---:|---:|---:|
| flat-40 | plain | 111 | 7.61 | 20.09 | **5.28** | 1.44 | 3.81 |
| flat-160 | plain | 512 | 52.98 | 117.91 | **23.83** | 2.22 | 4.95 |
| tree-5 | plain | 237 | 17.91 | 38.27 | **16.13** | 1.11 | 2.37 |
| tree-7 | plain | 757 | 66.21 | 136.68 | **56.60** | 1.17 | 2.41 |
| random-d6 | HoldForm | 19 | 1.22 | 4.23 | **0.79** | 1.54 | 5.33 |
| random-d8 | HoldForm | 29 | 1.73 | 6.63 | **1.21** | 1.43 | 5.50 |
| random-d10 | HoldForm | 34 | 2.35 | 10.66 | **1.54** | 1.53 | 6.93 |
| random-d6 | Inactive | 24 | 2.66 | 6.35 | **1.75** | 1.52 | 3.63 |
| random-d8 | Inactive | 40 | 4.23 | 10.69 | **3.14** | 1.35 | 3.41 |
| random-d10 | Inactive | 48 | 5.17 | 18.30 | **3.85** | 1.34 | 4.75 |

`TeX / Native` median 1.44× (min 1.11, max 2.22), 10/10 outputs identical to the
reference converter. The native config patch is memoized per configuration
(single-entry, value-keyed), so the ~1 ms per-call cost of diffing the ~800-entry
default character map is paid once, not on every conversion.

One run on the development machine; the 500+ leaf cases move by ±10–20% between
runs because `MakeBoxes` dominates and the GC is involved. The ratios are the
stable part.

`> 1` means native is faster. The `Inactive` rows exist for path coverage: every
`Inactive[f][…]` boxes to `TemplateBox[{…}, "InactiveHead"]`, which is the shape
that originally showed a 2× regression, and it is resolved from the generated
template table. The corpus proper is the `HoldForm` ones.
