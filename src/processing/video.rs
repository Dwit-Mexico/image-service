//! Pipeline de procesamiento de video vía `ffmpeg` (subprocess).
//!
//! Input: cualquier formato que ffmpeg pueda decodificar (mp4, mov, webm, ...).
//! Output:
//!   - video: MP4 H.264 (libx264) + AAC, escalado a `max_height` preservando
//!     aspecto, calidad CRF tunable.
//!   - thumbnail: WebP extraído del segundo 1 (o el último frame disponible
//!     si el video es más corto que 1s).
//!
//! Rechaza videos que excedan `max_duration_seconds` (default 120s) con un
//! error claro — evita que un cliente cuelgue el pod con un upload de horas.

use std::io::Write;
use std::process::Stdio;

use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::AppError;

const DEFAULT_MAX_HEIGHT: u32 = 720;
const DEFAULT_CRF: u8 = 24;
const DEFAULT_AUDIO_BITRATE_K: u32 = 128;
const DEFAULT_MAX_DURATION_SECS: f32 = 120.0;
const THUMBNAIL_SEEK: &str = "00:00:01.000";

#[derive(Debug, Clone, Deserialize)]
pub struct VideoOptions {
    #[serde(default)]
    pub max_height: Option<u32>,
    #[serde(default)]
    pub crf: Option<u8>,
    #[serde(default)]
    pub audio_bitrate_k: Option<u32>,
    #[serde(default)]
    pub max_duration_seconds: Option<f32>,
    #[serde(default)]
    pub folder: Option<String>,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            max_height: None,
            crf: None,
            audio_bitrate_k: None,
            max_duration_seconds: None,
            folder: None,
        }
    }
}

pub struct VideoResult {
    pub video_bytes: Vec<u8>,
    pub thumbnail_bytes: Vec<u8>,
    pub duration_seconds: f32,
}

/// Decodifica, transcodea y extrae thumbnail. Async porque spawn de ffmpeg
/// es I/O-bound; el work pesado es ejecución externa.
pub async fn process_video(raw: &[u8], opts: &VideoOptions) -> Result<VideoResult, AppError> {
    let max_height = opts.max_height.unwrap_or(DEFAULT_MAX_HEIGHT);
    let crf = opts.crf.unwrap_or(DEFAULT_CRF).clamp(0, 51);
    let audio_bitrate_k = opts.audio_bitrate_k.unwrap_or(DEFAULT_AUDIO_BITRATE_K);
    let max_duration_s = opts
        .max_duration_seconds
        .unwrap_or(DEFAULT_MAX_DURATION_SECS);

    let input = write_temp("video-in", raw)?;

    let duration = ffprobe_duration(input.path().to_str().ok_or_else(|| {
        AppError::Processing("ruta temporal contiene caracteres inválidos".into())
    })?)
    .await?;
    if duration > max_duration_s {
        return Err(AppError::BadRequest(format!(
            "video dura {duration:.1}s, máximo permitido {max_duration_s:.0}s"
        )));
    }

    let video_out = NamedTempFile::with_suffix(".mp4")
        .map_err(|e| AppError::Processing(format!("tempfile: {e}")))?;
    let thumb_out = NamedTempFile::with_suffix(".webp")
        .map_err(|e| AppError::Processing(format!("tempfile: {e}")))?;

    // Re-encode a H.264 + AAC. `scale='-2:min(ih,720)'` baja a 720p si supera,
    // mantiene aspect ratio, y -2 fuerza par (libx264 requiere dims pares).
    let scale_filter = format!("scale='-2:min(ih,{max_height})'");
    let crf_str = crf.to_string();
    let audio_bitrate = format!("{audio_bitrate_k}k");
    let video_args = [
        "-y",
        "-i",
        input.path().to_str().unwrap(),
        "-vf",
        &scale_filter,
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        &crf_str,
        "-c:a",
        "aac",
        "-b:a",
        &audio_bitrate,
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
        video_out.path().to_str().unwrap(),
    ];
    run_ffmpeg(&video_args).await?;

    // Thumbnail: si el video es más corto que THUMBNAIL_SEEK, intenta con
    // el último frame (`-sseof`); fallback al primer frame.
    let seek = if duration >= 1.0 {
        THUMBNAIL_SEEK.to_string()
    } else {
        "00:00:00.000".to_string()
    };
    let thumb_args = [
        "-y",
        "-ss",
        &seek,
        "-i",
        input.path().to_str().unwrap(),
        "-frames:v",
        "1",
        "-vf",
        "scale='-2:min(ih,720)'",
        "-c:v",
        "libwebp",
        "-q:v",
        "80",
        "-f",
        "webp",
        thumb_out.path().to_str().unwrap(),
    ];
    run_ffmpeg(&thumb_args).await?;

    let video_bytes = read_file(video_out.path()).await?;
    let thumbnail_bytes = read_file(thumb_out.path()).await?;
    Ok(VideoResult {
        video_bytes,
        thumbnail_bytes,
        duration_seconds: duration,
    })
}

async fn ffprobe_duration(path: &str) -> Result<f32, AppError> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::Processing(format!("ffprobe spawn: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::BadRequest(format!(
            "ffprobe rechazó el archivo: {}",
            stderr.lines().last().unwrap_or("error desconocido")
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .parse::<f32>()
        .map_err(|_| AppError::Processing(format!("ffprobe duration parse: {text:?}")))
}

async fn run_ffmpeg(args: &[&str]) -> Result<(), AppError> {
    let out = Command::new("ffmpeg")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::Processing(format!("ffmpeg spawn: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Processing(format!(
            "ffmpeg falló: {}",
            stderr.lines().last().unwrap_or("error desconocido")
        )));
    }
    Ok(())
}

fn write_temp(prefix: &str, bytes: &[u8]) -> Result<NamedTempFile, AppError> {
    let mut f = NamedTempFile::with_prefix(prefix)
        .map_err(|e| AppError::Processing(format!("tempfile: {e}")))?;
    f.write_all(bytes)
        .map_err(|e| AppError::Processing(format!("tempfile write: {e}")))?;
    f.flush()
        .map_err(|e| AppError::Processing(format!("tempfile flush: {e}")))?;
    Ok(f)
}

async fn read_file(path: &std::path::Path) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::Processing(format!("read tempfile: {e}")))?;
    f.read_to_end(&mut buf)
        .await
        .map_err(|e| AppError::Processing(format!("read tempfile: {e}")))?;
    Ok(buf)
}
