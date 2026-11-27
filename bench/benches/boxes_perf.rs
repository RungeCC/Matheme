//! Pure Rust Box → Typst conversion benchmark.
//!
//! Run with `cargo bench -p matheme-bench --bench boxes_perf`.
//! Set `RAYON_NUM_THREADS=1` to measure the sequential baseline of the same
//! parser.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use matheme_wl::{Expr, Symbol};
use std::hint::black_box;

fn expr(head: &str, args: Vec<Expr>) -> Expr {
    Expr::normal(Symbol::new(format!("System`{head}")), args)
}

fn row(items: Vec<Expr>) -> Expr {
    expr("RowBox", vec![Expr::normal(Symbol::LIST, items)])
}

fn deep_fractions(depth: usize) -> Expr {
    if depth == 0 {
        return Expr::string("x");
    }
    expr(
        "FractionBox",
        vec![deep_fractions(depth - 1), Expr::string("a")],
    )
}

fn wide_row(n: usize) -> Expr {
    row((0..n)
        .map(|i| {
            if i % 3 == 0 {
                expr("FractionBox", vec![Expr::string("a"), Expr::string("b")])
            } else {
                Expr::string(if i % 2 == 0 { "x" } else { "+" })
            }
        })
        .collect())
}

/// Many independent rows wrapped in one `List`, i.e. a top-level expression
/// whose top node is not itself a wide row. This is the case where the
/// parallelism decision cannot be made from a single sibling count.
fn many_rows(rows: usize, width: usize) -> Expr {
    expr(
        "List",
        (0..rows)
            .map(|i| {
                row((0..width)
                    .map(|j| {
                        if (i + j) % 3 == 0 {
                            expr("FractionBox", vec![Expr::string("a"), Expr::string("b")])
                        } else {
                            Expr::string("x")
                        }
                    })
                    .collect())
            })
            .collect(),
    )
}

/// One big fraction inside an otherwise small tree. The two halves together
/// cross the 32-node pair-join threshold while the whole tree stays far below
/// `PARALLEL_MIN_NODES`, so this shape must render sequentially.
fn fraction_pair_deep(depth: usize) -> Expr {
    expr(
        "FractionBox",
        vec![deep_fractions(depth), deep_fractions(depth)],
    )
}

fn matrix(rows: usize, cols: usize) -> Expr {
    expr(
        "GridBox",
        vec![Expr::normal(
            Symbol::LIST,
            (0..rows)
                .map(|r| {
                    Expr::normal(
                        Symbol::LIST,
                        (0..cols)
                            .map(|c| {
                                if (r + c) % 4 == 0 {
                                    expr("FractionBox", vec![Expr::string("a"), Expr::string("b")])
                                } else {
                                    Expr::string("x")
                                }
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
        )],
    )
}

fn boxes_perf(c: &mut Criterion) {
    let mut group = c.benchmark_group("boxes_to_typst");
    group.warm_up_time(std::time::Duration::from_millis(300));
    group.measurement_time(std::time::Duration::from_millis(800));

    for depth in [4_usize, 8, 12] {
        let tree = deep_fractions(depth);
        group.bench_with_input(
            BenchmarkId::new("deep_fraction", depth),
            &tree,
            |b, tree| b.iter(|| black_box(matheme_box::boxes_to_typst(tree).unwrap())),
        );
    }
    // Guards the regression where the pair-join path spawned rayon tasks
    // without consulting the tree-wide parallel budget.
    for depth in [8_usize, 10] {
        let tree = fraction_pair_deep(depth);
        group.bench_with_input(
            BenchmarkId::new("fraction_pair", depth),
            &tree,
            |b, tree| b.iter(|| black_box(matheme_box::boxes_to_typst(tree).unwrap())),
        );
    }
    for width in [8_usize, 64, 256, 1024, 4096] {
        let tree = wide_row(width);
        group.bench_with_input(BenchmarkId::new("wide_row", width), &tree, |b, tree| {
            b.iter(|| black_box(matheme_box::boxes_to_typst(tree).unwrap()))
        });
    }
    // Small vs large matrices: the 2x2 must stay sequential, the large one
    // (>= PARALLEL_MIN_NODES nodes and >= PARALLEL_MIN_ELEMS rows) is the case
    // row-level parallelism was built for.
    for (rows, cols) in [(2_usize, 2_usize), (128, 32)] {
        let tree = matrix(rows, cols);
        group.bench_with_input(
            BenchmarkId::new("matrix", format!("{rows}x{cols}")),
            &tree,
            |b, tree| b.iter(|| black_box(matheme_box::boxes_to_typst(tree).unwrap())),
        );
    }
    for (rows, width) in [(16_usize, 16_usize), (64, 32), (256, 32)] {
        let tree = many_rows(rows, width);
        group.bench_with_input(
            BenchmarkId::new("many_rows", format!("{rows}x{width}")),
            &tree,
            |b, tree| b.iter(|| black_box(matheme_box::boxes_to_typst(tree).unwrap())),
        );
    }
    group.finish();
}

criterion_group!(benches, boxes_perf);
criterion_main!(benches);
