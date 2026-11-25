//! Unit tests for Wolfram Language ``System`Graphics`` encoding.

use std::sync::Arc;

use matheme_wl::graphics;
use typst::foundations::Bytes;

use super::curves::GLYPH_EXPR_CACHE;
use super::*;
use crate::extract::{cache_generation, clear_glyph_cache};
use crate::ir::{
    ClipRegion, EmbeddedImageFormat, GlyphInstances, GlyphKey, GradientPaint, GradientSpread,
    GradientStop, ImagePayload, LineCapStyle, LineJoinStyle, PaintStyle, RasterPixels, Rgba,
    Segment, StrokeStyle, Subpath, TilingPaint, UnitGlyphKey, VectorPage, VectorPrimitive,
    WindingRule,
};
use crate::lru::lock_unpoisoned;

#[test]
fn encodes_filled_and_joined_curves_with_exact_options() {
    let subpath = Subpath {
        start: [0.0, 0.0],
        segments: vec![
            Segment::Line([10.0, 0.0]),
            Segment::Cubic([10.0, 5.0], [5.0, 10.0], [0.0, 10.0]),
            Segment::Line([0.0, 0.0]),
        ],
        closed: true,
    };
    let page = VectorPage {
        size: [10.5, 10.25],
        bounds: [[0.0, 10.5], [0.0, 10.25]],
        primitives: vec![VectorPrimitive::Paths {
            paths: vec![vec![subpath]],
            fill: Some(PaintStyle::Solid(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            })),
            stroke: None,
            winding: WindingRule::NonZero,
        }],
    };

    let expr = page_to_expr(&page);
    let normal = expr.as_normal().expect("Graphics[...]");
    assert!(normal.has_head(&graphics::GRAPHICS));
    let rendered = expr.to_string();
    assert!(rendered.contains("System`FilledCurve"), "{rendered}");
    assert!(rendered.contains("System`PlotRange"), "{rendered}");
    assert!(rendered.contains("System`PlotRangePadding"), "{rendered}");
    assert!(rendered.contains("System`ImagePadding"), "{rendered}");
    assert!(!rendered.contains("System`Polygon"), "{rendered}");
    assert!(rendered.contains("10.5, 10.25"), "{rendered}");
}

#[test]
fn encodes_rect_clip_path_clip_and_rotated_image_primitives() {
    let clipped_child = VectorPrimitive::Paths {
        paths: vec![vec![Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line([40.0, 0.0]),
                Segment::Line([40.0, 40.0]),
                Segment::Line([0.0, 40.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        }]],
        fill: Some(PaintStyle::Solid(Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        stroke: None,
        winding: WindingRule::EvenOdd,
    };
    let rect_clip = VectorPrimitive::Clip {
        region: ClipRegion::Rect([[10.0, 30.0], [10.0, 25.0]]),
        children: vec![clipped_child.clone()],
    };
    let tri_clip = VectorPrimitive::Clip {
        region: ClipRegion::Path {
            subpaths: vec![Subpath {
                start: [10.0, 10.0],
                segments: vec![
                    Segment::Line([30.0, 10.0]),
                    Segment::Line([20.0, 30.0]),
                    Segment::Line([10.0, 10.0]),
                ],
                closed: true,
            }],
            bounds: [[10.0, 30.0], [10.0, 30.0]],
            ctrl_bounds: [[10.0, 30.0], [10.0, 30.0]],
        },
        children: vec![clipped_child],
    };
    let encoded_img = VectorPrimitive::Image {
        payload: ImagePayload::Encoded {
            format: EmbeddedImageFormat::Png,
            data: Bytes::new([0x89, b'P', b'N', b'G']),
        },
        size: [20.0, 10.0],
        transform: [0.0, 1.0, -1.0, 0.0, 35.0, 5.0],
    };
    let page = VectorPage {
        size: [40.0, 40.0],
        bounds: [[0.0, 40.0], [0.0, 40.0]],
        primitives: vec![rect_clip, tri_clip, encoded_img],
    };
    let rendered = page_to_expr(&page).to_string();
    assert!(rendered.contains("System`Inset"), "{rendered}");
    assert!(rendered.contains("System`Texture"), "{rendered}");
    assert!(rendered.contains("System`Polygon[{}]"), "{rendered}");
    assert!(rendered.contains("System`Background"), "{rendered}");
    assert!(
        rendered.contains("System`Left, System`Bottom"),
        "{rendered}"
    );
    assert!(rendered.contains("System`PlotRangeClipping"), "{rendered}");
    assert!(rendered.contains("System`True"), "{rendered}");
    assert!(rendered.contains("System`ImportByteArray"), "{rendered}");
    assert!(rendered.contains("\"PNG\""), "{rendered}");
    assert!(
        rendered.contains("System`GeometricTransformation"),
        "{rendered}"
    );
}

#[test]
fn encodes_decoded_raster_as_bottom_up_numeric_array_without_import() {
    // 2x2 RGBA top-down: row 0 = [Red, Green], row 1 = [Blue, Yellow].
    let pixels = RasterPixels {
        width: 2,
        height: 2,
        channels: 4,
        data: Arc::from(vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 128,
        ]),
    };
    let page = VectorPage {
        size: [40.0, 30.0],
        bounds: [[0.0, 40.0], [0.0, 30.0]],
        primitives: vec![
            VectorPrimitive::Image {
                payload: ImagePayload::Raster(pixels.clone()),
                size: [20.0, 10.0],
                transform: [1.0, 0.0, 0.0, 1.0, 5.0, 8.0],
            },
            VectorPrimitive::Image {
                payload: ImagePayload::Raster(pixels),
                size: [20.0, 10.0],
                transform: [0.0, 1.0, -1.0, 0.0, 30.0, 4.0],
            },
        ],
    };

    let expr = page_to_expr(&page);
    let rendered = expr.to_string();
    assert!(rendered.contains("System`Raster"), "{rendered}");
    assert!(
        !rendered.contains("System`ImportByteArray"),
        "decoded rasters must bypass ImportByteArray: {rendered}"
    );

    // Inspect the NumericArray inside the first Raster primitive directly.
    let elems = expr.as_normal().unwrap().get(0).unwrap().as_list().unwrap();
    let raster0 = elems[0].as_normal().expect("Raster");
    assert!(raster0.has_head(&graphics::RASTER));
    let na = raster0
        .get(0)
        .unwrap()
        .as_numeric_array()
        .expect("NumericArray payload");
    assert_eq!(na.dimensions(), &[2, 2, 4]);
    let bytes = na.try_as_slice::<u8>().expect("u8 slice");
    // Bottom-up row 0 must be original row 1 (Blue, Yellow); row 1 must be Red, Green.
    assert_eq!(
        bytes,
        &[
            0, 0, 255, 255, 255, 255, 0, 128, 255, 0, 0, 255, 0, 255, 0, 255
        ]
    );

    // Second primitive is rotated -> wrapped in GeometricTransformation[Raster[...], ...].
    let gt = elems[1].as_normal().expect("GeometricTransformation");
    assert!(gt.has_head(&graphics::GEOMETRIC_TRANSFORMATION));
    assert!(
        gt.get(0)
            .unwrap()
            .as_normal()
            .unwrap()
            .has_head(&graphics::RASTER)
    );
}

#[test]
fn encodes_gradients_tiling_and_dash_phase_natively() {
    let box_sp = Subpath {
        start: [0.0, 0.0],
        segments: vec![
            Segment::Line([100.0, 0.0]),
            Segment::Line([100.0, 100.0]),
            Segment::Line([0.0, 100.0]),
            Segment::Line([0.0, 0.0]),
        ],
        closed: true,
    };
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        },
        GradientStop {
            offset: 1.0,
            color: Rgba {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            },
        },
    ];
    let page = VectorPage {
        size: [100.0, 100.0],
        bounds: [[0.0, 100.0], [0.0, 100.0]],
        primitives: vec![
            VectorPrimitive::Paths {
                paths: vec![vec![box_sp.clone()]],
                fill: Some(PaintStyle::Gradient(GradientPaint::Linear {
                    origin: [0.0, 0.0],
                    grad: [0.01, 0.0],
                    stops: stops.clone(),
                    spread: GradientSpread::Pad,
                })),
                stroke: None,
                winding: WindingRule::EvenOdd,
            },
            VectorPrimitive::Paths {
                paths: vec![vec![box_sp.clone()]],
                fill: Some(PaintStyle::Gradient(GradientPaint::Radial {
                    local_to_page: [100.0, 0.0, 0.0, -100.0, 0.0, 100.0],
                    center: [0.5, 0.5],
                    radius: 0.5,
                    focal_center: [0.5, 0.5],
                    focal_radius: 0.0,
                    stops: stops.clone(),
                    spread: GradientSpread::Pad,
                })),
                stroke: None,
                winding: WindingRule::EvenOdd,
            },
            VectorPrimitive::Paths {
                paths: vec![vec![box_sp.clone()]],
                fill: Some(PaintStyle::Gradient(GradientPaint::Radial {
                    local_to_page: [100.0, 0.0, 0.0, -100.0, 0.0, 100.0],
                    center: [0.5, 0.5],
                    radius: 0.5,
                    focal_center: [0.25, 0.3],
                    focal_radius: 0.05,
                    stops: stops.clone(),
                    spread: GradientSpread::Pad,
                })),
                stroke: None,
                winding: WindingRule::EvenOdd,
            },
            VectorPrimitive::Paths {
                paths: vec![vec![box_sp]],
                fill: Some(PaintStyle::Gradient(GradientPaint::Conic {
                    local_to_page: [100.0, 0.0, 0.0, -100.0, 0.0, 100.0],
                    container_size: [100.0, 100.0],
                    center: [0.5, 0.5],
                    angle: 0.0,
                    stops,
                })),
                stroke: None,
                winding: WindingRule::EvenOdd,
            },
            VectorPrimitive::Line {
                start: [0.0, 50.0],
                end: [100.0, 50.0],
                stroke: StrokeStyle {
                    color: Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                    thickness: 2.0,
                    cap: LineCapStyle::Round,
                    join: LineJoinStyle::Miter,
                    miter_limit: 4.0,
                    dash: Some((vec![10.0, 5.0], 3.0)),
                },
            },
        ],
    };
    let expr = page_to_expr(&page);
    let rendered = expr.to_string();
    assert!(
        rendered.contains("System`LinearGradientFilling"),
        "{rendered}"
    );
    assert!(
        rendered.contains("System`RadialGradientFilling"),
        "{rendered}"
    );
    assert!(
        rendered.contains("System`ConicGradientFilling"),
        "{rendered}"
    );
    assert!(
        rendered.contains("System`Texture[System`Image["),
        "{rendered}"
    );
    assert!(
        rendered.contains("System`Dashing[{0.1, 0.05}, 0.03, \"Round\"]"),
        "{rendered}"
    );

    // Off-center focal radial gradient uses NumericArray inside Texture[Image[...]].
    let elems = expr.as_normal().unwrap().get(0).unwrap().as_list().unwrap();
    // elems[0] is the sRGB sentinel, elems[1]=Linear, elems[2]=Radial, elems[3]=Off-center Radial.
    let focal_group = elems[3].as_list().expect("focal group");
    let tex = focal_group[1].as_normal().expect("Texture");
    assert!(tex.has_head(&graphics::TEXTURE));
    let img = tex.get(0).unwrap().as_normal().expect("Image");
    assert!(img.has_head(&graphics::IMAGE));
    let na = img
        .get(0)
        .unwrap()
        .as_numeric_array()
        .expect("NumericArray in gradient Texture");
    assert_eq!(na.dimensions(), &[96, 96, 4]);
}

#[test]
fn rectangular_conic_tiling_uses_pure_vector_inset_without_texture() {
    let tile_sp = Subpath {
        start: [0.0, 0.0],
        segments: vec![
            Segment::Line([30.0, 0.0]),
            Segment::Line([30.0, 30.0]),
            Segment::Line([0.0, 30.0]),
            Segment::Line([0.0, 0.0]),
        ],
        closed: true,
    };
    let rect_sp = Subpath {
        start: [10.0, 10.0],
        segments: vec![
            Segment::Line([440.0, 10.0]),
            Segment::Line([440.0, 90.0]),
            Segment::Line([10.0, 90.0]),
            Segment::Line([10.0, 10.0]),
        ],
        closed: true,
    };
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        },
        GradientStop {
            offset: 0.5,
            color: Rgba {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            },
        },
        GradientStop {
            offset: 1.0,
            color: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        },
    ];
    let tile_prim = VectorPrimitive::Paths {
        paths: vec![vec![tile_sp]],
        fill: Some(PaintStyle::Gradient(GradientPaint::Conic {
            local_to_page: [30.0, 0.0, 0.0, -30.0, 0.0, 30.0],
            container_size: [30.0, 30.0],
            center: [0.5, 0.5],
            angle: 0.0,
            stops,
        })),
        stroke: None,
        winding: WindingRule::EvenOdd,
    };
    let tiling = TilingPaint {
        tile_size: [30.0, 30.0],
        step: [40.0, 50.0],
        offset: [0.0, 0.0],
        grid_to_page: [1.0, 0.0, 0.0, -1.0, 10.0, 90.0],
        primitives: vec![tile_prim],
    };
    let page = VectorPage {
        size: [450.0, 100.0],
        bounds: [[0.0, 450.0], [0.0, 100.0]],
        primitives: vec![VectorPrimitive::Paths {
            paths: vec![vec![rect_sp]],
            fill: Some(PaintStyle::Tiling(Box::new(tiling))),
            stroke: Some(StrokeStyle {
                color: Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
                thickness: 1.0,
                cap: LineCapStyle::Butt,
                join: LineJoinStyle::Miter,
                miter_limit: 4.0,
                dash: Some((vec![1.0, 2.0], 0.0)),
            }),
            winding: WindingRule::EvenOdd,
        }],
    };
    let rendered = page_to_expr(&page).to_string();
    assert!(rendered.contains("System`Inset"), "{rendered}");
    assert!(
        rendered.contains("System`ConicGradientFilling"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("System`Texture"),
        "axis-aligned rectangular tiling must not rasterize via Texture: {rendered}"
    );
}

#[test]
fn glyph_expr_cache_invalidates_on_world_cache_generation() {
    let key = GlyphKey {
        unit: UnitGlyphKey {
            font_gen: 1,
            font_hash: 0x1234_5678,
            face_index: 0,
            variations_hash: 0,
            glyph_id: 42,
        },
        linear_q: [10_000_000, 0, 0, 10_000_000],
    };
    let inst = GlyphInstances {
        key,
        subpaths: Arc::new(vec![Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line([5.0, 0.0]),
                Segment::Line([5.0, 5.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        }]),
        positions: vec![[0.0, 0.0]],
    };

    let encoder = WlEncoder::new();
    let _ = encoder.cached_glyph_expr(&inst, false);
    assert!(
        lock_unpoisoned(&GLYPH_EXPR_CACHE)
            .entries
            .contains_key(&key)
    );
    clear_glyph_cache();
    let (_, joined) = encoder.cached_glyph_expr(&inst, true);
    assert!(joined.is_some());
    assert_eq!(
        lock_unpoisoned(&GLYPH_EXPR_CACHE).generation,
        cache_generation()
    );
}

#[test]
fn sub_point_page_preserves_image_size_aspect_ratio() {
    let page = VectorPage {
        size: [0.5, 0.25],
        bounds: [[0.0, 0.5], [0.0, 0.25]],
        primitives: Vec::new(),
    };
    let rendered = page_to_expr(&page).to_string();
    assert!(
        rendered.contains("0.5, 0.25"),
        "expected exact sub-pt ImageSize: {rendered}"
    );
}
