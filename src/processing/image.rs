use std::io::Cursor;

use image::{imageops::FilterType, DynamicImage, ImageFormat};
use serde::Deserialize;

use crate::error::AppError;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Webp,
    Jpeg,
    Png,
}

impl OutputFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Webp => "webp",
            OutputFormat::Jpeg => "jpg",
            OutputFormat::Png => "png",
        }
    }

    pub fn mime(&self) -> &'static str {
        match self {
            OutputFormat::Webp => "image/webp",
            OutputFormat::Jpeg => "image/jpeg",
            OutputFormat::Png => "image/png",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessOptions {
    /// Calidad 1-100. Default: 85
    pub quality: Option<u8>,
    /// Ancho máximo en px. Default: 2048
    pub max_width: Option<u32>,
    /// Formato de salida. Default: webp
    pub format: Option<OutputFormat>,
    /// Contenedor Azure destino. Default: env DEFAULT_CONTAINER
    pub container: Option<String>,
    /// Carpeta dentro del contenedor. Ej: "users/123/avatars"
    pub folder: Option<String>,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            quality: Some(85),
            max_width: Some(2048),
            format: Some(OutputFormat::Webp),
            container: None,
            folder: None,
        }
    }
}

/// Decodifica, redimensiona y convierte una imagen al formato destino.
/// Devuelve (bytes_comprimidos, formato).
pub fn process_image(
    raw: &[u8],
    opts: &ProcessOptions,
) -> Result<(Vec<u8>, OutputFormat), AppError> {
    let quality = opts.quality.unwrap_or(85).clamp(1, 100);
    let max_width = opts.max_width.unwrap_or(2048);
    let format = opts.format.clone().unwrap_or_default();

    // Decodificar — soporta JPEG, PNG, GIF, BMP, TIFF, ICO
    let img = image::load_from_memory(raw)
        .map_err(|e| AppError::Processing(format!("formato no soportado: {e}")))?;

    // Redimensionar si supera max_width (preserva aspect ratio)
    let img = resize_if_needed(img, max_width);

    let encoded = encode(&img, &format, quality)?;
    Ok((encoded, format))
}

fn resize_if_needed(img: DynamicImage, max_width: u32) -> DynamicImage {
    if img.width() <= max_width {
        return img;
    }
    let new_height = (img.height() as f64 * max_width as f64 / img.width() as f64) as u32;
    img.resize_exact(max_width, new_height, FilterType::Lanczos3)
}

fn encode(img: &DynamicImage, format: &OutputFormat, quality: u8) -> Result<Vec<u8>, AppError> {
    match format {
        OutputFormat::Webp => encode_webp(img, quality),
        OutputFormat::Jpeg => encode_jpeg(img, quality),
        OutputFormat::Png => encode_png(img),
    }
}

/// 🔴 `webp::Encoder::from_image` **solo** acepta `Rgb8` y `Rgba8`: cualquier
/// otra cosa —escala de grises, o los 16 bits por canal con los que iOS guarda
/// algunas capturas de pantalla— devuelve `Err("Unimplemented")` y el upload
/// respondia 422. Se normaliza el color antes de encodear, igual que ya hacia
/// `encode_jpeg` con su `to_rgb8()`; el formato de salida sigue siendo webp.
fn encode_webp(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, AppError> {
    let data = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        webp::Encoder::from_rgba(rgba.as_raw(), w, h).encode(quality as f32).to_vec()
    } else {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        webp::Encoder::from_rgb(rgb.as_raw(), w, h).encode(quality as f32).to_vec()
    };
    Ok(data)
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, AppError> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .encode_image(&rgb)
        .map_err(|e| AppError::Processing(format!("jpeg encoder: {e}")))?;
    Ok(buf)
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, AppError> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| AppError::Processing(format!("png encoder: {e}")))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 La regresion del reporte 118: una captura de iPhone en escala de
    /// grises (o de 16 bits) reventaba con `webp encoder: Unimplemented` y
    /// nadie podia adjuntar imagenes en los comentarios. Aqui se cubren los dos
    /// tipos de pixel que el crate de webp rechaza, encodeando el camino real.
    #[test]
    fn encodea_webp_lo_que_no_es_rgb8() {
        use image::{GrayImage, ImageBuffer, Luma, Rgba};

        let gris = DynamicImage::ImageLuma8(GrayImage::from_pixel(40, 30, Luma([128])));
        let webp_gris = encode_webp(&gris, 85).expect("gris debe encodear");
        assert!(webp_gris.starts_with(b"RIFF"), "no salio un webp valido");

        let profundo: ImageBuffer<Rgba<u16>, Vec<u16>> =
            ImageBuffer::from_pixel(40, 30, Rgba([30000, 20000, 10000, 65535]));
        let webp_16 = encode_webp(&DynamicImage::ImageRgba16(profundo), 85)
            .expect("16 bits debe encodear");
        assert!(webp_16.starts_with(b"RIFF"), "no salio un webp valido");
    }
}
