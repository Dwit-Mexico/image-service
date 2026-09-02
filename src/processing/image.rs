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
    /// Alto máximo en px. Default: sin tope.
    ///
    /// Con sólo `max_width`, una foto vertical se acota únicamente de ancho:
    /// 3000×4000 con `max_width: 2048` se guarda 2048×2731, más alta de lo que
    /// nadie pidió. Dando los dos, la imagen se ajusta para **caber dentro** de
    /// la caja preservando el aspect ratio.
    ///
    /// El default es sin tope a propósito: así el comportamiento de quien sólo
    /// manda `max_width` no cambia.
    pub max_height: Option<u32>,
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
            max_height: None,
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
    let max_height = opts.max_height.unwrap_or(u32::MAX);
    let format = opts.format.clone().unwrap_or_default();

    // Decodificar — soporta JPEG, PNG, GIF, BMP, TIFF, ICO
    let img = image::load_from_memory(raw)
        .map_err(|e| AppError::Processing(format!("formato no soportado: {e}")))?;

    // Redimensionar para caber en la caja (preserva aspect ratio)
    let img = resize_if_needed(img, max_width, max_height);

    let encoded = encode(&img, &format, quality)?;
    Ok((encoded, format))
}

fn resize_if_needed(img: DynamicImage, max_width: u32, max_height: u32) -> DynamicImage {
    if img.width() <= max_width && img.height() <= max_height {
        return img;
    }
    // `resize` (no `resize_exact`) escala al mayor tamaño que quepa dentro de la
    // caja conservando el aspect ratio, que es justo lo que hacía a mano el
    // cálculo anterior de `new_height` — pero mirando también el alto.
    img.resize(max_width, max_height, FilterType::Lanczos3)
}

fn encode(img: &DynamicImage, format: &OutputFormat, quality: u8) -> Result<Vec<u8>, AppError> {
    match format {
        OutputFormat::Webp => encode_webp(img, quality),
        OutputFormat::Jpeg => encode_jpeg(img, quality),
        OutputFormat::Png => encode_png(img),
    }
}

fn encode_webp(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, AppError> {
    let encoder = webp::Encoder::from_image(img)
        .map_err(|e| AppError::Processing(format!("webp encoder: {e}")))?;
    let data = encoder.encode(quality as f32);
    Ok(data.to_vec())
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
    use image::RgbImage;

    fn img(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::new(w, h))
    }

    #[test]
    fn no_toca_lo_que_ya_cabe() {
        let out = resize_if_needed(img(800, 600), 2048, 2048);
        assert_eq!((out.width(), out.height()), (800, 600));
    }

    #[test]
    fn horizontal_se_acota_de_ancho() {
        let out = resize_if_needed(img(4000, 3000), 2048, u32::MAX);
        assert_eq!((out.width(), out.height()), (2048, 1536));
    }

    /// La regresión que importa: sin `max_height` el comportamiento tiene que
    /// ser idéntico al de antes — una vertical sólo se acota de ancho y queda
    /// más alta que el tope de ancho.
    #[test]
    fn sin_max_height_la_vertical_queda_alta() {
        let out = resize_if_needed(img(3000, 4000), 2048, u32::MAX);
        assert_eq!((out.width(), out.height()), (2048, 2731));
    }

    #[test]
    fn con_max_height_la_vertical_cabe_en_la_caja() {
        let out = resize_if_needed(img(3000, 4000), 2048, 2048);
        assert_eq!((out.width(), out.height()), (1536, 2048));
        assert!(out.width() <= 2048 && out.height() <= 2048);
    }

    /// Sólo excede el alto: antes esto no se tocaba porque nadie miraba el alto.
    #[test]
    fn se_acota_aunque_el_ancho_ya_quepa() {
        let out = resize_if_needed(img(1000, 4000), 2048, 2048);
        assert_eq!((out.width(), out.height()), (512, 2048));
    }

    /// Los de arriba prueban el helper. Éste prueba el camino real —decodificar,
    /// redimensionar y encodear a webp, que es lo que corre en producción— para
    /// fijar que `process_image` de verdad lee la opción nueva y no sólo que el
    /// helper sabe usarla.
    #[test]
    fn process_image_respeta_la_caja() {
        let mut raw = Vec::new();
        img(600, 800)
            .write_to(&mut Cursor::new(&mut raw), ImageFormat::Jpeg)
            .expect("encodear el jpeg de prueba");

        let opts = ProcessOptions {
            max_width: Some(400),
            max_height: Some(400),
            ..ProcessOptions::default()
        };
        let (bytes, format) = process_image(&raw, &opts).expect("procesar");

        assert!(matches!(format, OutputFormat::Webp));
        let out = image::load_from_memory(&bytes).expect("decodificar la salida");
        assert_eq!((out.width(), out.height()), (300, 400));
    }

    #[test]
    fn el_default_no_pone_tope_de_alto() {
        let d = ProcessOptions::default();
        assert_eq!(d.max_width, Some(2048));
        assert_eq!(d.max_height, None);
    }
}
