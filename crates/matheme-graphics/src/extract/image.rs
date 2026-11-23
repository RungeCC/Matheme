//! Embedded image extraction: SVG vectorization, decoded 8-bit raster pixel
//! extraction (preserving Typst's EXIF rotation without PNG re-encoding), and
//! PDF fallback.

use std::sync::Arc;

use image::DynamicImage;
use kurbo::Affine;
use typst::layout::{Abs, Axes, Ratio, Transform};
use typst::visualize::{Image, ImageKind, RasterImage};
use typst_svg::WebImage;

use super::Extractor;
use crate::geometry::{affine_from_typst, include_transformed_box, BoundsTracker};
use crate::ir::{EmbeddedImageFormat, ImagePayload, RasterPixels, VectorPrimitive};

/// Extract top-down 8-bit row-major pixels directly from Typst's already-decoded
/// [`RasterImage::dynamic`] (which already has EXIF orientation applied).
pub(super) fn extract_raster_pixels(raster: &RasterImage) -> Option<RasterPixels> {
    let dynamic = raster.dynamic();
    let (width, height, channels, raw): (u32, u32, u8, Vec<u8>) = match dynamic.as_ref() {
        DynamicImage::ImageLuma8(img) => (img.width(), img.height(), 1, img.as_raw().clone()),
        DynamicImage::ImageLumaA8(img) => (img.width(), img.height(), 2, img.as_raw().clone()),
        DynamicImage::ImageRgb8(img) => (img.width(), img.height(), 3, img.as_raw().clone()),
        DynamicImage::ImageRgba8(img) => (img.width(), img.height(), 4, img.as_raw().clone()),
        other if other.color().has_alpha() => {
            let rgba = other.to_rgba8();
            (rgba.width(), rgba.height(), 4, rgba.into_raw())
        }
        other => {
            let rgb = other.to_rgb8();
            (rgb.width(), rgb.height(), 3, rgb.into_raw())
        }
    };
    if width == 0 || height == 0 {
        return None;
    }
    Some(RasterPixels {
        width,
        height,
        channels,
        data: Arc::from(raw),
    })
}

impl Extractor {
    pub(super) fn extract_image(
        &mut self,
        image: &Image,
        size: &Axes<Abs>,
        ts: Transform,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        self.active_glyph_indices.clear();
        let w = size.x.to_pt().max(1e-6);
        let h = size.y.to_pt().max(1e-6);

        match image.kind() {
            ImageKind::Svg(svg_img) => {
                // Typst's layout engine has already resolved `image(fit: ...)` into
                // `size`, so mapping `tree.size()` onto `(w, h)` composed with the
                // full 6-parameter affine `ts` preserves rotation, skew, and fit.
                let tree = svg_img.tree();
                let tree_size = tree.size();
                let sx = w / f64::from(tree_size.width()).max(1e-6);
                let sy = h / f64::from(tree_size.height()).max(1e-6);
                let canvas_to_page = affine_from_typst(ts) * Affine::scale_non_uniform(sx, sy);
                self.extract_usvg_group(tree.root(), canvas_to_page, 1.0, bbox, primitives);
            }
            ImageKind::Raster(raster) => {
                let Some(pixels) = extract_raster_pixels(raster) else {
                    return;
                };
                let img_ts = ts
                    .pre_concat(Transform::translate(Abs::zero(), size.y))
                    .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
                let img_affine = affine_from_typst(img_ts);
                include_transformed_box(w, h, img_affine, bbox);
                primitives.push(VectorPrimitive::Image {
                    payload: ImagePayload::Raster(pixels),
                    size: [w, h],
                    transform: img_affine.as_coeffs(),
                });
            }
            ImageKind::Pdf(_) => {
                let img_ts = ts
                    .pre_concat(Transform::translate(Abs::zero(), size.y))
                    .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
                let img_affine = affine_from_typst(img_ts);
                include_transformed_box(w, h, img_affine, bbox);

                let web = WebImage::new(image);
                let Some(format) = EmbeddedImageFormat::from_extension(web.format.extension())
                else {
                    self.warnings.unsupported_image = true;
                    return;
                };
                primitives.push(VectorPrimitive::Image {
                    payload: ImagePayload::Encoded {
                        format,
                        data: web.data,
                    },
                    size: [w, h],
                    transform: img_affine.as_coeffs(),
                });
            }
        }
    }
}
