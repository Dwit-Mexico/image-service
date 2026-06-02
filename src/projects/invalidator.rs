//! Subscriber Valkey que invalida el cache local cuando otro pod o un
//! endpoint admin publica un cambio.
//!
//! Canal: `projects:invalidate`. Payload: el `cert_cn` que cambió.
//! Publicación de `*` (literal) → invalida todo (útil para "reload all").
//!
//! El servicio sigue funcionando si Valkey está caído o no configurado —
//! el TTL local del cache es la red de seguridad. Por eso el subscriber
//! es opcional (cargado solo si VALKEY_SENTINEL_ADDR está en el env) y
//! hace reconexión con backoff exponencial sin matar el proceso.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use redis::sentinel::{Sentinel, SentinelNodeConnectionInfo};
use redis::{Client, RedisConnectionInfo};
use tokio::time::sleep;

use crate::projects::ProjectResolver;

pub const CHANNEL: &str = "projects:invalidate";

pub struct ValkeyConfig {
    pub sentinel_addrs: Vec<String>,
    pub master_name: String,
    pub password: Option<String>,
}

impl ValkeyConfig {
    /// Devuelve `None` si Valkey no está configurado. En local (sin acceso
    /// a la red interna de k8s) el servicio arranca sin pub/sub.
    pub fn from_env() -> Option<Self> {
        let addrs = std::env::var("VALKEY_SENTINEL_ADDR").ok()?;
        Some(Self {
            sentinel_addrs: addrs.split(',').map(|s| s.trim().to_string()).collect(),
            master_name: std::env::var("VALKEY_MASTER_NAME")
                .unwrap_or_else(|_| "mymaster".to_string()),
            password: std::env::var("VALKEY_PASSWORD").ok(),
        })
    }
}

/// Conecta y se queda suscrito al canal. Reconecta con backoff si cae.
pub async fn run_subscriber(resolver: Arc<ProjectResolver>, cfg: ValkeyConfig) -> ! {
    let mut backoff = Duration::from_secs(1);
    loop {
        match subscribe_once(&resolver, &cfg).await {
            Ok(()) => {
                tracing::info!("valkey subscriber stream cerró — reconectando");
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                tracing::warn!("valkey subscriber error: {e}; retry en {backoff:?}");
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn subscribe_once(
    resolver: &ProjectResolver,
    cfg: &ValkeyConfig,
) -> Result<(), redis::RedisError> {
    let master = master_client(cfg).await?;
    let mut pubsub = master.get_async_pubsub().await?;
    pubsub.subscribe(CHANNEL).await?;
    tracing::info!("valkey subscriber listo en canal '{CHANNEL}'");

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = msg.get_payload().unwrap_or_default();
        if payload == "*" {
            tracing::info!("invalidando todo el cache");
            resolver.invalidate_all().await;
        } else if !payload.is_empty() {
            tracing::debug!("invalidando cert_cn='{payload}'");
            resolver.invalidate(&payload).await;
        }
    }
    Ok(())
}

/// Descubre el master actual vía Sentinel y devuelve un `Client` listo.
async fn master_client(cfg: &ValkeyConfig) -> Result<Client, redis::RedisError> {
    let sentinel_urls: Vec<String> = cfg
        .sentinel_addrs
        .iter()
        .map(|addr| sentinel_url(addr, cfg.password.as_deref()))
        .collect();

    let mut sentinel = Sentinel::build(sentinel_urls)?;
    let node_conn = SentinelNodeConnectionInfo {
        tls_mode: None,
        redis_connection_info: Some(RedisConnectionInfo {
            db: 0,
            username: None,
            password: cfg.password.clone(),
            protocol: redis::ProtocolVersion::RESP2,
        }),
    };
    sentinel
        .async_master_for(&cfg.master_name, Some(&node_conn))
        .await
}

fn sentinel_url(addr: &str, password: Option<&str>) -> String {
    match password {
        Some(p) => format!("redis://:{}@{addr}", urlencoding::encode(p)),
        None => format!("redis://{addr}"),
    }
}

/// Publica una invalidación. Si Valkey está caído, el caller debe loggear
/// y seguir — los otros pods cogerán el cambio al expirar el TTL.
pub async fn publish_invalidation(
    cfg: &ValkeyConfig,
    cert_cn: &str,
) -> Result<(), redis::RedisError> {
    let client = master_client(cfg).await?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    redis::cmd("PUBLISH")
        .arg(CHANNEL)
        .arg(cert_cn)
        .query_async::<()>(&mut conn)
        .await
}
