//! Embedded raster & container image encoding for Wolfram `Graphics`.
//!
//! Decoded Typst raster images are emitted directly as native
//! `Raster[NumericArray[...], rect, {0, 255}]` primitives (optionally wrapped
//! in `GeometricTransformation` for rotation/skew/reflection). Over WSTP,
//! `NumericArray` travels as a single contiguous WXF byte array
//! (`BinaryDeserialize[ByteArray[...]]`), avoiding both per-pixel expression
//! allocation and Wolfram's slow `ImportByteArray` pipeline.

use matheme_wl::{Expr, fundamental, graphics, list, normal};

use super::WlEncoder;
use super::curves::point_expr;
use crate::ir::{EmbeddedImageFormat, ImagePayload, RasterPixels};

impl WlEncoder {
    pub(super) fn image_to_expr(
        &self,
        payload: &ImagePayload,
        size: [f64; 2],
        transform: [f64; 6],
    ) -> Expr {
        match payload {
            ImagePayload::Raster(pixels) => raster_image_to_expr(pixels, size, transform),
            ImagePayload::Encoded { format, data } => {
                self.encoded_image_to_expr(*format, data.as_ref(), size, transform)
            }
        }
    }

    fn encoded_image_to_expr(
        &self,
        format: EmbeddedImageFormat,
        data: &[u8],
        size: [f64; 2],
        transform: [f64; 6],
    ) -> Expr {
        let imported = normal!(
            &graphics::IMPORT_BYTE_ARRAY,
            Expr::bytes(data),
            wl_image_format(format),
        );
        let stretched = normal!(
            &graphics::SHOW,
            imported,
            &graphics::ASPECT_RATIO => &fundamental::FULL,
        );
        let [sx, ky, kx, sy, tx, ty] = transform;

        if kx.abs() < 1e-9 && ky.abs() < 1e-9 && sx > 0.0 && sy > 0.0 {
            normal!(
                &graphics::INSET,
                stretched,
                point_expr([tx, ty]),
                self.origin_left_bottom.clone(),
                list![size[0] * sx, size[1] * sy],
            )
        } else {
            let inset = normal!(
                &graphics::INSET,
                stretched,
                point_expr([0.0, 0.0]),
                self.origin_left_bottom.clone(),
                list![size[0], size[1]],
            );
            let matrix = list![list![sx, kx], list![ky, sy]];
            normal!(
                &graphics::GEOMETRIC_TRANSFORMATION,
                inset,
                list![matrix, point_expr([tx, ty])],
            )
        }
    }
}

/// Convert top-down [`RasterPixels`] into a bottom-up `UnsignedInteger8`
/// `NumericArray` suitable for Wolfram's `Raster[..., rect, {0, 255}]`.
fn raster_pixels_to_bottom_up_numeric_array(pixels: &RasterPixels) -> Expr {
    let w = pixels.width as usize;
    let h = pixels.height as usize;
    let ch = pixels.channels as usize;
    let row_bytes = w * ch;

    let mut bottom_up = Vec::with_capacity(h * row_bytes);
    for row in (0..h).rev() {
        let start = row * row_bytes;
        bottom_up.extend_from_slice(&pixels.data[start..start + row_bytes]);
    }

    if ch == 1 {
        Expr::numeric_array_u8(&[h, w], &bottom_up)
    } else {
        Expr::numeric_array_u8(&[h, w, ch], &bottom_up)
    }
}

fn raster_image_to_expr(pixels: &RasterPixels, size: [f64; 2], transform: [f64; 6]) -> Expr {
    let na = raster_pixels_to_bottom_up_numeric_array(pixels);
    let [sx, ky, kx, sy, tx, ty] = transform;
    let range = list![0_i64, 255_i64];

    if kx.abs() < 1e-9 && ky.abs() < 1e-9 && sx > 0.0 && sy > 0.0 {
        let bounds = list![
            point_expr([tx, ty]),
            point_expr([tx + size[0] * sx, ty + size[1] * sy]),
        ];
        normal!(&graphics::RASTER, na, bounds, range)
    } else {
        let bounds = list![point_expr([0.0, 0.0]), point_expr(size)];
        let raster = normal!(&graphics::RASTER, na, bounds, range);
        let matrix = list![list![sx, kx], list![ky, sy]];
        normal!(
            &graphics::GEOMETRIC_TRANSFORMATION,
            raster,
            list![matrix, point_expr([tx, ty])],
        )
    }
}

/// Canonical format name accepted by Wolfram Language's `ImportByteArray`.
fn wl_image_format(format: EmbeddedImageFormat) -> &'static str {
    match format {
        EmbeddedImageFormat::Png => "PNG",
        EmbeddedImageFormat::Jpg => "JPEG",
        EmbeddedImageFormat::Gif => "GIF",
        EmbeddedImageFormat::Webp => "WebP",
        EmbeddedImageFormat::Svg => "SVG",
    }
}
