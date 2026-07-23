//! Pipeline de procesamiento de audio vía `ffmpeg` (subprocess).
//!
//! Input: cualquier formato que ffmpeg pueda decodificar (wav, m4a, flac, ogg, ...).
//! Output: MP3 (libmp3lame) con bitrate tunable.
//!
//! Rechaza audios que excedan `max_duration_seconds` (default 180s).

use std::io::Write;
use std::process::Stdio;

use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::AppError;

const DEFAULT_BITRATE_K: u32 = 128;
const DEFAULT_MAX_DURATION_SECS: f32 = 180.0;

#[derive(Debug, Clone, Deserialize)]
pub struct AudioOptions {
    #[serde(default)]
    pub bitrate_k: Option<u32>,
    #[serde(default)]
    pub max_duration_seconds: Option<f32>,
    #[serde(default)]
    pub folder: Option<String>,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            bitrate_k: None,
            max_duration_seconds: None,
            folder: None,
        }
    }
}

pub struct AudioResult {
    pub bytes: Vec<u8>,
    pub duration_seconds: f32,
}

pub async fn process_audio(raw: &[u8], opts: &AudioOptions) -> Result<AudioResult, AppError> {
    let bitrate_k = opts.bitrate_k.unwrap_or(DEFAULT_BITRATE_K);
    let max_duration_s = opts
        .max_duration_seconds
        .unwrap_or(DEFAULT_MAX_DURATION_SECS);

    let input = write_temp("audio-in", raw)?;
    let path = input.path().to_str().ok_or_else(|| {
        AppError::Processing("ruta temporal contiene caracteres inválidos".into())
    })?;
    // ffprobe `format=duration` viene vacío/`N/A` en el WebM/Opus que graba el
    // MediaRecorder del navegador (grabación en vivo, sin duración en el header).
    // En ese caso medimos decodificando el archivo completo.
    let duration = match ffprobe_duration(path).await {
        Ok(d) if d.is_finite() && d > 0.0 => d,
        _ => decode_duration(path).await?,
    };
    if duration > max_duration_s {
        return Err(AppError::BadRequest(format!(
            "audio dura {duration:.1}s, máximo permitido {max_duration_s:.0}s"
        )));
    }

    let out = NamedTempFile::with_suffix(".mp3")
        .map_err(|e| AppError::Processing(format!("tempfile: {e}")))?;

    let bitrate = format!("{bitrate_k}k");
    let args = [
        "-y",
        "-i",
        input.path().to_str().unwrap(),
        "-vn", // sin pista de video si el input la trae
        "-c:a",
        "libmp3lame",
        "-b:a",
        &bitrate,
        "-f",
        "mp3",
        out.path().to_str().unwrap(),
    ];
    run_ffmpeg(&args).await?;

    let bytes = read_file(out.path()).await?;
    Ok(AudioResult {
        bytes,
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

/// Mide la duración decodificando el archivo completo con ffmpeg. Se usa como
/// fallback cuando `ffprobe format=duration` no reporta duración (típico del
/// WebM/Opus de MediaRecorder). `-progress pipe:1` emite `out_time_us=` (µs) en
/// stdout; tomamos el último valor.
async fn decode_duration(path: &str) -> Result<f32, AppError> {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path,
            "-f",
            "null",
            "-progress",
            "pipe:1",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::Processing(format!("ffmpeg spawn: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::BadRequest(format!(
            "ffmpeg no pudo decodificar el audio: {}",
            stderr.lines().last().unwrap_or("error desconocido")
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `out_time_us`/`out_time_ms` de ffmpeg vienen en microsegundos (el nombre
    // `ms` es un histórico engañoso). Tomamos el último reportado.
    let micros = stdout.lines().rev().find_map(|l| {
        l.strip_prefix("out_time_us=")
            .or_else(|| l.strip_prefix("out_time_ms="))
            .and_then(|v| v.trim().parse::<f64>().ok())
    });
    if let Some(us) = micros {
        if us > 0.0 {
            return Ok((us / 1_000_000.0) as f32);
        }
    }
    Err(AppError::Processing(
        "no se pudo medir la duración del audio".into(),
    ))
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
