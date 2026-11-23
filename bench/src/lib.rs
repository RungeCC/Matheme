//! Shared Typst document workloads and helpers for Matheme's Criterion
//! benchmarks (`bench/benches/`) and performance regression tests
//! (`bench/tests/`).

use std::path::PathBuf;

use matheme_world::MathemeWorld;
use typst::foundations::Dict;
use typst_layout::PagedDocument;

/// Compile a Typst source string into a [`PagedDocument`] for benchmarking.
pub fn compile_doc(source: &str) -> PagedDocument {
    let world = MathemeWorld::new(source.to_string(), Dict::new(), PathBuf::from("."));
    typst::compile::<PagedDocument>(&world)
        .output
        .expect("benchmark Typst source should compile")
}

/// 200 non-intersecting closed cubic contours (`fill-rule: "non-zero"`).
pub fn doc_winding_fast_curves() -> String {
    let mut s = String::from("#set page(width: 420pt, height: 420pt, margin: 10pt)\n");
    for i in 0..200 {
        let x = (i % 20) * 20;
        let y = (i / 20) * 20;
        s.push_str(&format!(
            "#place(dx: {x}pt, dy: {y}pt, curve(fill: blue, fill-rule: \"non-zero\", \
             curve.move((0pt, 8pt)), \
             curve.cubic((0pt, 0pt), (16pt, 0pt), (16pt, 8pt)), \
             curve.cubic((16pt, 16pt), (0pt, 16pt), (0pt, 8pt)), \
             curve.close()))\n"
        ));
    }
    s
}

/// 500 non-intersecting linear triangles & quads (`fill-rule: "non-zero"`).
pub fn doc_winding_simple_polys() -> String {
    let mut s = String::from("#set page(width: 500pt, height: 500pt, margin: 10pt)\n");
    for i in 0..500 {
        let x = (i % 25) * 18;
        let y = (i / 25) * 18;
        if i % 2 == 0 {
            s.push_str(&format!(
                "#place(dx: {x}pt, dy: {y}pt, polygon(fill: teal, fill-rule: \"non-zero\", \
                 (0pt, 0pt), (14pt, 0pt), (7pt, 14pt)))\n"
            ));
        } else {
            s.push_str(&format!(
                "#place(dx: {x}pt, dy: {y}pt, polygon(fill: orange, fill-rule: \"non-zero\", \
                 (0pt, 0pt), (14pt, 0pt), (14pt, 14pt), (0pt, 14pt)))\n"
            ));
        }
    }
    s
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// `star_count` self-intersecting star polygons (`points`-pointed stars).
pub fn doc_winding_slow_stars(star_count: usize, points: usize) -> String {
    let mut s = String::from("#set page(width: 420pt, height: 420pt, margin: 10pt)\n");
    let mut step = (points / 2).max(2);
    while step > 2 && gcd(step, points) != 1 {
        step -= 1;
    }
    for idx in 0..star_count {
        let ox = (idx % 4) * 95 + 45;
        let oy = (idx / 4) * 95 + 45;
        s.push_str("#place(dx: 0pt, dy: 0pt, polygon(fill: rgb(\"#2563eb\"), fill-rule: \"non-zero\"");
        for k in 0..points {
            let v = (k * step) % points;
            let angle = std::f64::consts::TAU * (v as f64) / (points as f64);
            let x = ox as f64 + 38.0 * angle.cos();
            let y = oy as f64 + 38.0 * angle.sin();
            s.push_str(&format!(", ({x:.3}pt, {y:.3}pt)"));
        }
        s.push_str("))\n");
    }
    s
}

/// Single high-vertex self-intersecting polygon (tests sweep-line scaling &
/// vertex/edge budget protection).
pub fn doc_winding_dense_self_intersecting(num_vertices: usize) -> String {
    let mut s = String::from(
        "#set page(width: 240pt, height: 240pt, margin: 10pt)\n\
         #polygon(fill: rgb(\"#dc2626\"), fill-rule: \"non-zero\"",
    );
    let step = 3;
    let n = if num_vertices.is_multiple_of(3) {
        num_vertices + 1
    } else {
        num_vertices
    };
    for k in 0..n {
        let v = (k * step) % n;
        let angle = std::f64::consts::TAU * (v as f64) / (n as f64);
        let r = 80.0 + 18.0 * (5.0 * angle).sin();
        let x = 110.0 + r * angle.cos();
        let y = 110.0 + r * angle.sin();
        s.push_str(&format!(", ({x:.3}pt, {y:.3}pt)"));
    }
    s.push_str(")\n");
    s
}

/// 600 stroked lines, axis-aligned rects, and curves.
pub fn doc_stroked_shapes() -> String {
    let mut s = String::from("#set page(width: 520pt, height: 520pt, margin: 10pt)\n");
    for i in 0..200 {
        let x = (i % 20) * 24;
        let y = (i / 20) * 24;
        s.push_str(&format!(
            "#place(dx: {x}pt, dy: {y}pt, line(length: 18pt, angle: {}deg, stroke: (paint: blue, thickness: 1.5pt, cap: \"round\")))\n",
            (i * 17) % 180
        ));
        s.push_str(&format!(
            "#place(dx: {x}pt, dy: {}pt, rect(width: 16pt, height: 12pt, stroke: 1.2pt + red))\n",
            y + 240
        ));
        s.push_str(&format!(
            "#place(dx: {x}pt, dy: {y}pt, circle(radius: 7pt, stroke: 1.0pt + green))\n"
        ));
    }
    s
}

/// Heavy multi-size + multi-glyph document producing >4,900 unique `GlyphKey`s
/// interleaved with a hot working set of common math & Latin glyphs.
pub fn doc_glyph_cache_churn(size_steps: usize) -> String {
    let mut s = String::from("#set page(width: 480pt, height: 640pt, margin: 12pt)\n");
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let hot = "The quick brown fox jumps over the lazy dog. $integral_0^1 x^2 dif x = 1/3$";
    for i in 0..size_steps {
        let sz = 6.0 + (i as f64) * 0.13;
        s.push_str(&format!("#text(size: {sz:.2}pt)[{alphabet}]\n"));
        s.push_str(&format!("#text(size: 10pt)[{hot}]\n"));
    }
    s
}

/// Rich 4-page academic document (math + prose + lists + shapes + clips).
pub fn doc_academic_multipage() -> String {
    r##"#set page(width: 420pt, height: 520pt, margin: 20pt)
#set text(size: 10pt)
#for sec in range(4) [
  = Section #(sec + 1): Variational Formulation

  Consider the energy functional $E[u] = integral_Omega (1/2 |nabla u|^2 - f u) dif x$
  subject to Dirichlet boundary conditions on $partial Omega$. The Euler-Lagrange
  equation yields $-Delta u = f$ with weak solution $u in H_0^1(Omega)$.

  $ sum_(n=1)^infinity 1/n^2 = pi^2/6, quad
    integral_(-infinity)^infinity e^(-a x^2) dif x = sqrt(pi / a), quad
    mat(1, 2, 3; 4, 5, 6; 7, 8, 9) $

  #box(width: 120pt, height: 24pt, clip: true,
    rect(width: 160pt, height: 40pt, fill: rgb("#dbeafe"), stroke: 1.5pt + rgb("#2563eb"))
  )

  #for row in range(14) [
    - Item #(row + 1) in section #(sec + 1): residual $r_#(row) = (row + 1) / (sec + 2)$ with error bound $O(h^2)$.
  ]
  #if sec < 3 [ #pagebreak() ]
]
"##
    .to_string()
}

/// Heavy multi-page document with definitions for `Evaluate` benchmarks (PF-03).
pub fn doc_compile_evaluate() -> String {
    let mut s = String::from(
        "#set page(width: 420pt, height: 320pt, margin: 18pt)\n\
         #let fib(n) = { let a = 0; let b = 1; for _ in range(n) { let t = a + b; a = b; b = t }; a }\n\
         #let table_data = range(40).map(i => (id: i, sq: i * i, cube: i * i * i))\n",
    );
    for p in 0..6 {
        s.push_str(&format!(
            "= Page {p}\n\
             #for i in range(18) [\n\
               - Row #i on page {p}: $sum_(k=0)^i k^2 = #(calc.quo(i * (i + 1) * (2 * i + 1), 6))$\n\
             ]\n\
             #pagebreak()\n"
        ));
    }
    s
}
