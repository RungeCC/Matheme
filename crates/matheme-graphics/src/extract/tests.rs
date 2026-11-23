//! Unit tests for Typst vector/raster extraction.

use std::path::PathBuf;

use matheme_world::MathemeWorld;
use typst::foundations::Dict;
use typst_layout::PagedDocument;

use super::*;
use crate::ir::GradientPaint;

fn compile_doc(src: &str) -> PagedDocument {
    let world = MathemeWorld::new(src.into(), Dict::new(), PathBuf::from("."));
    typst::compile::<PagedDocument>(&world)
        .output
        .expect("typst compile")
}

fn compile_typst(src: &str) -> VectorExport {
    compile_typst_with_gen(src, 1)
}

fn compile_typst_with_gen(src: &str, font_gen: u64) -> VectorExport {
    let doc = compile_doc(src);
    export_vector(&doc, None, font_gen)
}

#[test]
fn extracts_glyph_outlines_and_expands_descender_bbox() {
    let export =
        compile_typst("#set page(width: auto, height: auto, margin: 0pt, fill: none)\nxxyyzz");
    assert_eq!(export.pages.len(), 1);
    let page = &export.pages[0];
    assert!(page.size[0] > 20.0);
    assert!(page.size[1] > 5.0);
    assert!(
        page.bounds[1][0] < -1.0,
        "expected negative y_min for 'y' descender, got {:?}",
        page.bounds
    );
    let (unique_glyphs, total_instances): (usize, usize) = page
        .primitives
        .iter()
        .map(|p| match p {
            VectorPrimitive::Glyphs { glyphs, .. } => {
                (glyphs.len(), glyphs.iter().map(|g| g.positions.len()).sum())
            }
            _ => (0, 0),
        })
        .fold((0, 0), |(u, t), (du, dt)| (u + du, t + dt));
    assert_eq!(unique_glyphs, 3);
    assert_eq!(total_instances, 6);
}

#[test]
fn extracts_unicode_math_curves_not_tofu() {
    let export = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$ x^y + integral_0^infinity e^(-x^2) dif x $",
    );
    assert_eq!(export.pages.len(), 1);
    let cubic_count: usize = export.pages[0]
        .primitives
        .iter()
        .flat_map(|p| match p {
            VectorPrimitive::Glyphs { glyphs, .. } => glyphs
                .iter()
                .flat_map(|g| g.subpaths.as_slice())
                .collect::<Vec<_>>(),
            VectorPrimitive::Paths { paths, .. } => paths.iter().flatten().collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .flat_map(|sp| &sp.segments)
        .filter(|seg| matches!(seg, Segment::Cubic(..)))
        .count();
    assert!(
        cubic_count > 50,
        "expected curved glyph outlines for unicode-math, got {cubic_count}"
    );
}

#[test]
fn expands_bounds_for_stroked_rect_and_emits_clip_primitive() {
    let stroked = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 40pt, height: 25pt, stroke: 4pt + red)",
    );
    let b = stroked.pages[0].bounds;
    assert!(
        (b[0][0] - (-2.0)).abs() < 0.05 && (b[0][1] - 42.0).abs() < 0.05,
        "stroke x bounds must match exact [-2, 42]: {b:?}"
    );
    assert!(
        (b[1][0] - (-2.0)).abs() < 0.05 && (b[1][1] - 27.0).abs() < 0.05,
        "stroke y bounds must match exact [-2, 27]: {b:?}"
    );

    let clipped = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#box(width: 20pt, height: 15pt, clip: true, rect(width: 60pt, height: 60pt, fill: red))",
    );
    let cb = clipped.pages[0].bounds;
    assert!(
        (cb[0][1] - cb[0][0] - 20.0).abs() < 0.01,
        "clipped width: {cb:?}"
    );
    assert!(
        (cb[1][1] - cb[1][0] - 15.0).abs() < 0.01,
        "clipped height: {cb:?}"
    );
    assert!(
        clipped.pages[0]
            .primitives
            .iter()
            .any(|p| matches!(p, VectorPrimitive::Clip { .. })),
        "expected VectorPrimitive::Clip"
    );
}

#[test]
fn unknown_image_extension_returns_none_instead_of_png() {
    assert_eq!(
        EmbeddedImageFormat::from_extension("png"),
        Some(EmbeddedImageFormat::Png)
    );
    assert_eq!(
        EmbeddedImageFormat::from_extension("JPEG"),
        Some(EmbeddedImageFormat::Jpg)
    );
    assert_eq!(EmbeddedImageFormat::from_extension("bmp"), None);
    assert_eq!(EmbeddedImageFormat::from_extension("tiff"), None);
}

#[test]
fn extracts_raw_raster_image_directly_as_decoded_pixels() {
    let export = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #image(bytes((255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 255, 255, 0, 255)), \
                format: (encoding: \"rgba8\", width: 2, height: 2), width: 20pt, height: 10pt, fit: \"stretch\")",
    );
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    let img = export.pages[0]
        .primitives
        .iter()
        .find_map(|p| match p {
            VectorPrimitive::Image { payload, size, .. } => Some((payload, *size)),
            _ => None,
        })
        .expect("expected VectorPrimitive::Image");
    assert!((img.1[0] - 20.0).abs() < 1e-6 && (img.1[1] - 10.0).abs() < 1e-6);
    match img.0 {
        ImagePayload::Raster(pixels) => {
            assert_eq!(pixels.width, 2);
            assert_eq!(pixels.height, 2);
            assert_eq!(pixels.channels, 4);
            assert_eq!(pixels.data.len(), 16);
            assert_eq!(&pixels.data[..4], &[255, 0, 0, 255]);
        }
        other => panic!("expected decoded Raster payload, got {other:?}"),
    }
}

#[test]
fn extracts_gradients_tilings_dash_phase_and_non_rect_clip_without_warnings() {
    let doc = compile_doc("#set page(width: auto, height: auto, margin: 0pt, fill: none)\nHello");
    let _ = export_vector(&doc, None, 101);
    let c1 = cache_generation();

    let grad = compile_typst_with_gen(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
        102,
    );
    assert!(cache_generation() > c1);
    assert!(
        grad.warnings.is_empty(),
        "unexpected warnings: {:?}",
        grad.warnings
    );
    assert!(
        grad.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Paths {
                fill: Some(PaintStyle::Gradient(GradientPaint::Linear { .. })),
                ..
            }
        )),
        "expected GradientPaint::Linear primitive"
    );

    let radial = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#circle(radius: 15pt, fill: gradient.radial(white, black))",
    );
    assert!(
        radial.warnings.is_empty(),
        "unexpected warnings: {:?}",
        radial.warnings
    );
    assert!(radial.pages[0].primitives.iter().any(|p| matches!(
        p,
        VectorPrimitive::Paths {
            fill: Some(PaintStyle::Gradient(GradientPaint::Radial { .. })),
            ..
        }
    )));

    let conic = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#circle(radius: 15pt, fill: gradient.conic(red, green, blue, red))",
    );
    assert!(
        conic.warnings.is_empty(),
        "unexpected warnings: {:?}",
        conic.warnings
    );
    assert!(conic.pages[0].primitives.iter().any(|p| matches!(
        p,
        VectorPrimitive::Paths {
            fill: Some(PaintStyle::Gradient(GradientPaint::Conic { .. })),
            ..
        }
    )));

    let tiling = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#let p = tiling(size: (10pt, 10pt), circle(radius: 3pt, fill: green))\n#rect(width: 40pt, height: 40pt, fill: p)",
    );
    assert!(
        tiling.warnings.is_empty(),
        "unexpected warnings: {:?}",
        tiling.warnings
    );
    assert!(tiling.pages[0].primitives.iter().any(|p| matches!(
        p,
        VectorPrimitive::Paths {
            fill: Some(PaintStyle::Tiling(_)),
            ..
        }
    )));

    let dashed = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#line(length: 40pt, stroke: (paint: black, thickness: 2pt, dash: (array: (6pt, 3pt), phase: 2pt)))",
    );
    assert!(
        dashed.warnings.is_empty(),
        "unexpected warnings: {:?}",
        dashed.warnings
    );
    assert!(dashed.pages[0].primitives.iter().any(|p| matches!(
        p,
        VectorPrimitive::Line { stroke, .. }
            if stroke.dash.as_ref().is_some_and(|(_, ph)| (*ph - 2.0).abs() < 1e-6)
    )));

    let non_rect_clip = compile_typst(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#box(width: 30pt, height: 30pt, radius: 10pt, clip: true, rect(width: 30pt, height: 30pt, fill: red))",
    );
    assert!(
        non_rect_clip.warnings.is_empty(),
        "unexpected warnings: {:?}",
        non_rect_clip.warnings
    );
    assert!(non_rect_clip.pages[0].primitives.iter().any(|p| matches!(
        p,
        VectorPrimitive::Clip {
            region: ClipRegion::Path { .. },
            ..
        }
    )));
}

#[test]
fn winding_budget_exceeded_emits_degradation_warning() {
    let mut src = String::from(
        "#set page(width: 200pt, height: 200pt, margin: 0pt)\n#polygon(fill: blue, fill-rule: \"non-zero\"",
    );
    let n = 1501;
    for k in 0..n {
        let v = (k * 4) % n;
        let a = std::f64::consts::TAU * (v as f64) / (n as f64);
        src.push_str(&format!(
            ", ({:.2}pt, {:.2}pt)",
            100.0 + 80.0 * a.cos(),
            100.0 + 80.0 * a.sin()
        ));
    }
    src.push(')');
    let export = compile_typst(&src);
    assert!(
        export
            .warnings
            .contains(&DegradationWarning::NonZeroWindingApproximated),
        "expected NonZeroWindingApproximated warning, got {:?}",
        export.warnings
    );
}
