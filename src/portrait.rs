//! Strict loader for the single uncompressed KTX1 portrait used by WebGPU.
//!
//! The private JPEG is a local build input only. The browser fetches one
//! top-down, uncompressed RGBA8 KTX level and uploads those bytes directly to
//! an sRGB WebGPU texture; no browser image decoder or TypeScript bridge is
//! involved.

use sib::render::{RenderError, RenderResult};

pub const PORTRAIT_KTX_URL: &str = "assets/textures/pooya.ktx";

const KTX1_IDENTIFIER: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'1', b'1', 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
const KTX1_HEADER_SIZE: usize = 64;
const KTX_ENDIAN_LITTLE: u32 = 0x0403_0201;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_RGBA: u32 = 0x1908;
const GL_SRGB8_ALPHA8: u32 = 0x8C43;
const MAX_DIMENSION: u32 = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaKtxImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaKtxImage {
    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

pub fn parse_ktx1_rgba8(bytes: &[u8]) -> RenderResult<RgbaKtxImage> {
    parse_ktx1_rgba8_asset(bytes, "pooya.ktx")
}

pub fn parse_ktx1_rgba8_asset(bytes: &[u8], asset_name: &str) -> RenderResult<RgbaKtxImage> {
    if bytes.len() < KTX1_HEADER_SIZE + 4 {
        return Err(RenderError::message(format!(
            "{asset_name} is shorter than a KTX1 header"
        )));
    }
    if bytes[..KTX1_IDENTIFIER.len()] != KTX1_IDENTIFIER {
        return Err(RenderError::message(format!(
            "{asset_name} has an invalid KTX1 identifier"
        )));
    }

    let endianness = read_u32(bytes, 12)?;
    if endianness != KTX_ENDIAN_LITTLE {
        return Err(RenderError::message(format!(
            "{asset_name} must use little-endian KTX1 fields"
        )));
    }
    let gl_type = read_u32(bytes, 16)?;
    let gl_type_size = read_u32(bytes, 20)?;
    let gl_format = read_u32(bytes, 24)?;
    let gl_internal_format = read_u32(bytes, 28)?;
    let gl_base_internal_format = read_u32(bytes, 32)?;
    if gl_type != GL_UNSIGNED_BYTE
        || gl_type_size != 1
        || gl_format != GL_RGBA
        || gl_internal_format != GL_SRGB8_ALPHA8
        || gl_base_internal_format != GL_RGBA
    {
        return Err(RenderError::message(format!(
            "{asset_name} must be uncompressed sRGB RGBA8"
        )));
    }

    let width = read_u32(bytes, 36)?;
    let height = read_u32(bytes, 40)?;
    let depth = read_u32(bytes, 44)?;
    let array_elements = read_u32(bytes, 48)?;
    let faces = read_u32(bytes, 52)?;
    let mip_levels = read_u32(bytes, 56)?;
    let key_value_bytes = read_u32(bytes, 60)? as usize;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(RenderError::message(format!(
            "{asset_name} has invalid dimensions {width}x{height}",
        )));
    }
    if depth != 0 || array_elements != 0 || faces != 1 || mip_levels != 1 {
        return Err(RenderError::message(format!(
            "{asset_name} must contain one 2D face and exactly one mip level"
        )));
    }
    if !key_value_bytes.is_multiple_of(4) {
        return Err(RenderError::message(format!(
            "{asset_name} metadata must be aligned to four bytes"
        )));
    }

    let level_header = KTX1_HEADER_SIZE
        .checked_add(key_value_bytes)
        .ok_or_else(|| RenderError::message(format!("{asset_name} metadata offset overflowed")))?;
    let image_start = level_header
        .checked_add(4)
        .ok_or_else(|| RenderError::message(format!("{asset_name} image offset overflowed")))?;
    let image_size = read_u32(bytes, level_header)? as usize;
    let expected_size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            RenderError::message(format!("{asset_name} dimensions overflowed RGBA storage"))
        })?;
    if image_size != expected_size {
        return Err(RenderError::message(format!(
            "{asset_name} level is {image_size} bytes; expected {expected_size}",
        )));
    }
    let image_end = image_start
        .checked_add(image_size)
        .ok_or_else(|| RenderError::message(format!("{asset_name} image range overflowed")))?;
    let rgba = bytes
        .get(image_start..image_end)
        .ok_or_else(|| RenderError::message(format!("{asset_name} image payload is truncated")))?
        .to_vec();
    if image_end != bytes.len() {
        return Err(RenderError::message(format!(
            "{asset_name} must contain exactly one image payload"
        )));
    }

    Ok(RgbaKtxImage {
        width,
        height,
        rgba,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> RenderResult<u32> {
    let encoded = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| RenderError::message("KTX1 header is truncated"))?;
    Ok(u32::from_le_bytes(encoded.try_into().map_err(|_| {
        RenderError::message("KTX1 field width is invalid")
    })?))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_default_portrait() -> RenderResult<RgbaKtxImage> {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(manifest_dir.join(PORTRAIT_KTX_URL)).map_err(|error| {
        RenderError::message(format!("failed to read {PORTRAIT_KTX_URL}: {error}"))
    })?;
    parse_ktx1_rgba8(&bytes)
}

#[cfg(target_arch = "wasm32")]
pub async fn load_default_portrait() -> RenderResult<RgbaKtxImage> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window =
        web_sys::window().ok_or_else(|| RenderError::message("browser window is unavailable"))?;
    let response = JsFuture::from(window.fetch_with_str(PORTRAIT_KTX_URL))
        .await
        .map_err(|error| {
            RenderError::message(format!("failed to fetch {PORTRAIT_KTX_URL}: {error:?}"))
        })?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| {
        RenderError::message(format!("fetch for {PORTRAIT_KTX_URL} returned no Response",))
    })?;
    if !response.ok() {
        return Err(RenderError::message(format!(
            "failed to fetch {PORTRAIT_KTX_URL}: HTTP {}",
            response.status(),
        )));
    }
    let buffer = response.array_buffer().map_err(|error| {
        RenderError::message(format!("failed to read {PORTRAIT_KTX_URL}: {error:?}",))
    })?;
    let buffer = JsFuture::from(buffer).await.map_err(|error| {
        RenderError::message(format!("failed to read {PORTRAIT_KTX_URL}: {error:?}",))
    })?;
    parse_ktx1_rgba8(&js_sys::Uint8Array::new(&buffer).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_ktx() -> Vec<u8> {
        let rgba = [
            9_u8, 18, 27, 255, // pixel 0
            36, 45, 54, 128, // pixel 1
        ];
        let fields = [
            KTX_ENDIAN_LITTLE,
            GL_UNSIGNED_BYTE,
            1,
            GL_RGBA,
            GL_SRGB8_ALPHA8,
            GL_RGBA,
            2,
            1,
            0,
            0,
            1,
            1,
            0,
        ];
        let mut bytes = KTX1_IDENTIFIER.to_vec();
        for field in fields {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        bytes.extend_from_slice(&(rgba.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&rgba);
        bytes
    }

    #[test]
    fn parses_top_down_uncompressed_rgba8_ktx1() {
        let image = parse_ktx1_rgba8(&tiny_ktx()).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [9, 18, 27, 255, 36, 45, 54, 128]);
        assert_eq!(image.aspect_ratio(), 2.0);
    }

    #[test]
    fn rejects_non_ktx_linear_and_compressed_payloads() {
        assert!(parse_ktx1_rgba8(b"not a JPEG or a KTX file").is_err());

        let mut linear = tiny_ktx();
        linear[28..32].copy_from_slice(&0x8058_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&linear).is_err());

        let mut compressed = tiny_ktx();
        compressed[16..20].copy_from_slice(&0_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&compressed).is_err());
    }

    #[test]
    fn rejects_invalid_layout_and_truncated_or_extra_data() {
        let mut wrong_endian = tiny_ktx();
        wrong_endian[12..16].copy_from_slice(&0x0102_0304_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&wrong_endian).is_err());

        let mut zero_width = tiny_ktx();
        zero_width[36..40].copy_from_slice(&0_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&zero_width).is_err());

        let mut mipmapped = tiny_ktx();
        mipmapped[56..60].copy_from_slice(&2_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&mipmapped).is_err());

        let mut misaligned_metadata = tiny_ktx();
        misaligned_metadata[60..64].copy_from_slice(&1_u32.to_le_bytes());
        assert!(parse_ktx1_rgba8(&misaligned_metadata).is_err());

        let mut truncated = tiny_ktx();
        truncated.pop();
        assert!(parse_ktx1_rgba8(&truncated).is_err());

        let mut trailing = tiny_ktx();
        trailing.extend_from_slice(&[0, 0, 0, 0]);
        assert!(parse_ktx1_rgba8(&trailing).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_portrait_is_a_matted_square_ktx() {
        let image = load_default_portrait().unwrap();
        assert_eq!((image.width, image.height), (512, 512));
        assert_eq!(image.aspect_ratio(), 1.0);

        let alpha = image.rgba.chunks_exact(4).map(|pixel| pixel[3]);
        let (transparent, subject) = alpha.fold((0_usize, 0_usize), |counts, value| {
            (
                counts.0 + usize::from(value == 0),
                counts.1 + usize::from(value >= 250),
            )
        });
        assert!(
            transparent > 100_000,
            "portrait background must be transparent"
        );
        assert!(
            subject > 100_000,
            "portrait subject matte must be substantial"
        );
    }
}
