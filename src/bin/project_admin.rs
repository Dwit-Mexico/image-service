//! CLI de operador para gestionar la tabla `projects` sin tocar SQL.
//!
//! Subcomandos:
//!   list                                       — listar proyectos activos
//!   show <cert_cn>                             — detalles de un proyecto
//!   create-azure <name> <cert_cn> <conn> [default_container]
//!   create-s3    <name> <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]
//!   rotate-storage-azure <cert_cn> <conn>      — cambia connection_string
//!   rotate-storage-s3    <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]
//!   revoke <cert_cn>                           — marca revoked_at = now()
//!   rotate-key <cert_cn>                       — genera api key nueva, devuelve plaintext una vez
//!
//! Requiere DATABASE_URL + MASTER_KEY_V1 en el entorno.
//! Si VALKEY_SENTINEL_ADDR + VALKEY_PASSWORD están definidos, publica
//! invalidaciones del cache a los otros pods al rotar.

use std::env;
use std::process::ExitCode;

use image_service::{
    admin::auth as admin_auth,
    crypto::{seal, Kek},
    projects::{api_key, invalidator, repo, storage_config, StorageConfig},
    storage,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = match args.first() {
        Some(c) => c.as_str(),
        None => {
            print_help();
            return ExitCode::FAILURE;
        }
    };
    let rest = &args[1..];

    // Comandos que no necesitan DB (se manejan antes para no exigir DATABASE_URL)
    match cmd {
        "help" | "-h" | "--help" => {
            print_help();
            return ExitCode::SUCCESS;
        }
        "admin-hash" => {
            return match cmd_admin_hash() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    let pool = match connect_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("DB connect failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result: Result<(), String> = match cmd {
        "list" => cmd_list(&pool).await,
        "show" => cmd_show(&pool, rest).await,
        "create-azure" => cmd_create_azure(&pool, rest).await,
        "create-s3" => cmd_create_s3(&pool, rest).await,
        "rotate-storage-azure" => cmd_rotate_storage_azure(&pool, rest).await,
        "rotate-storage-s3" => cmd_rotate_storage_s3(&pool, rest).await,
        "revoke" => cmd_revoke(&pool, rest).await,
        "rotate-key" => cmd_rotate_key(&pool, rest).await,
        other => Err(format!("comando desconocido: {other}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!(
        "uso: project-admin <comando> [args]\n\n\
         comandos:\n  \
           list\n  \
           show <cert_cn>\n  \
           create-azure         <name> <cert_cn> <connection_string> [default_container]\n  \
           create-s3            <name> <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]\n  \
           rotate-storage-azure <cert_cn> <connection_string> [default_container]\n  \
           rotate-storage-s3    <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]\n  \
           revoke               <cert_cn>\n  \
           rotate-key           <cert_cn>\n  \
           admin-hash           (lee password de stdin, imprime hash argon2id)"
    );
}

async fn connect_pool() -> Result<PgPool, sqlx::Error> {
    let url = env::var("DATABASE_URL").expect("DATABASE_URL missing");
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
}

fn load_kek() -> Result<Kek, String> {
    Kek::from_env().map_err(|e| format!("MASTER_KEY_V1: {e}"))
}

async fn cmd_list(pool: &PgPool) -> Result<(), String> {
    let rows = repo::list_all(pool).await.map_err(|e| e.to_string())?;
    if rows.is_empty() {
        println!("(no projects)");
        return Ok(());
    }
    println!(
        "{:<10} {:<28} {:<14} {:<8} {:<14} {:<20} status",
        "name", "cert_cn", "prefix", "backend", "container", "last_used"
    );
    for r in rows {
        let status = match r.revoked_at {
            Some(_) => "REVOKED",
            None => "active",
        };
        println!(
            "{:<10} {:<28} {:<14} {:<8} {:<14} {:<20} {}",
            truncate(&r.name, 10),
            truncate(&r.cert_cn, 28),
            r.api_key_prefix,
            r.storage_backend,
            truncate(r.default_container.as_deref().unwrap_or(""), 14),
            r.last_used_at
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".into()),
            status
        );
    }
    Ok(())
}

async fn cmd_show(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let cert_cn = args.first().ok_or("uso: show <cert_cn>")?;
    let row = repo::find_active_by_cert_cn(pool, cert_cn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("proyecto '{cert_cn}' no encontrado"))?;
    println!("id              {}", row.id);
    println!("name            {}", row.name);
    println!("cert_cn         {}", row.cert_cn);
    println!("api_key_prefix  {}", row.api_key_prefix);
    println!("storage_backend {}", row.storage_backend);
    println!("default_container {:?}", row.default_container);
    println!("created_at      {}", row.created_at);
    println!("last_used_at    {:?}", row.last_used_at);
    println!("(storage credentials están cifradas y no se imprimen)");
    Ok(())
}

async fn cmd_create_azure(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or("uso: create-azure <name> <cert_cn> <connection_string> [default_container]")?;
    let cert_cn = args.get(1).ok_or("falta cert_cn")?;
    let conn = args.get(2).ok_or("falta connection_string")?;
    let default_container = args.get(3).map(|s| s.as_str());

    let cfg = StorageConfig::Azure {
        connection_string: conn.clone(),
    };
    insert_project(pool, name, cert_cn, "azure", &cfg, default_container).await
}

async fn cmd_create_s3(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or(
        "uso: create-s3 <name> <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]",
    )?;
    let cert_cn = args.get(1).ok_or("falta cert_cn")?;
    let access = args.get(2).ok_or("falta access_key")?;
    let secret = args.get(3).ok_or("falta secret_key")?;
    let region = args.get(4).ok_or("falta region")?;
    let bucket = args.get(5).ok_or("falta bucket")?;
    let endpoint = args.get(6).cloned();

    let cfg = StorageConfig::S3 {
        access_key_id: access.clone(),
        secret_access_key: secret.clone(),
        region: region.clone(),
        bucket: bucket.clone(),
        endpoint,
    };
    insert_project(pool, name, cert_cn, "s3", &cfg, None).await
}

async fn insert_project(
    pool: &PgPool,
    name: &str,
    cert_cn: &str,
    backend: &str,
    cfg: &StorageConfig,
    default_container: Option<&str>,
) -> Result<(), String> {
    storage::validate(cfg)?;
    let kek = load_kek()?;
    let key = api_key::generate();
    let storage_json = storage_config::to_json(cfg).map_err(|e| e.to_string())?;
    let blob = seal(&kek, &storage_json).map_err(|e| e.to_string())?;

    let id = repo::insert(
        pool,
        repo::NewProject {
            name,
            cert_cn,
            api_key_hash: &key.hash,
            storage_backend: backend,
            storage_blob: &blob,
            default_container,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    println!("created project");
    println!("  id        {id}");
    println!("  name      {name}");
    println!("  cert_cn   {cert_cn}");
    println!("  backend   {backend}");
    println!();
    println!("API key (MUÉSTRALA UNA SOLA VEZ — no se puede recuperar):");
    println!("  {}", key.plaintext);
    Ok(())
}

async fn cmd_rotate_storage_azure(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let cert_cn = args
        .first()
        .ok_or("uso: rotate-storage-azure <cert_cn> <connection_string> [default_container]")?;
    let conn = args.get(1).ok_or("falta connection_string")?;
    let default_container = args.get(2).map(|s| s.as_str());

    let cfg = StorageConfig::Azure {
        connection_string: conn.clone(),
    };
    rotate(pool, cert_cn, "azure", &cfg, default_container).await
}

async fn cmd_rotate_storage_s3(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let cert_cn = args.first().ok_or(
        "uso: rotate-storage-s3 <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]",
    )?;
    let access = args.get(1).ok_or("falta access_key")?;
    let secret = args.get(2).ok_or("falta secret_key")?;
    let region = args.get(3).ok_or("falta region")?;
    let bucket = args.get(4).ok_or("falta bucket")?;
    let endpoint = args.get(5).cloned();

    let cfg = StorageConfig::S3 {
        access_key_id: access.clone(),
        secret_access_key: secret.clone(),
        region: region.clone(),
        bucket: bucket.clone(),
        endpoint,
    };
    rotate(pool, cert_cn, "s3", &cfg, None).await
}

async fn rotate(
    pool: &PgPool,
    cert_cn: &str,
    backend: &str,
    cfg: &StorageConfig,
    default_container: Option<&str>,
) -> Result<(), String> {
    storage::validate(cfg)?;
    let kek = load_kek()?;
    let row = repo::find_active_by_cert_cn(pool, cert_cn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("proyecto '{cert_cn}' no encontrado"))?;

    let storage_json = storage_config::to_json(cfg).map_err(|e| e.to_string())?;
    let blob = seal(&kek, &storage_json).map_err(|e| e.to_string())?;

    let ok = repo::rotate_storage(pool, row.id, backend, &blob, default_container)
        .await
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err("rotate no afectó filas".into());
    }
    println!("rotated storage for {cert_cn} (id={})", row.id);
    println!("  backend = {backend}");
    if let Some(c) = default_container {
        println!("  default_container = {c}");
    }
    publish_invalidation_best_effort(cert_cn).await;
    Ok(())
}

/// Si Valkey está configurado, publica la invalidación para que los otros pods
/// purguen su cache al instante. Si falla (Valkey down, no configurado), solo
/// loggea — el TTL de 30s del cache local se encarga eventualmente.
async fn publish_invalidation_best_effort(cert_cn: &str) {
    let Some(cfg) = invalidator::ValkeyConfig::from_env() else {
        eprintln!("(sin VALKEY_SENTINEL_ADDR — cache local de otros pods expira en ~30s)");
        return;
    };
    match invalidator::publish_invalidation(&cfg, cert_cn).await {
        Ok(_) => eprintln!("invalidación publicada a Valkey — otros pods purgan ya"),
        Err(e) => eprintln!("(warning: publish a Valkey falló: {e}; TTL de 30s lo cubre)"),
    }
}

async fn cmd_revoke(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let cert_cn = args.first().ok_or("uso: revoke <cert_cn>")?;
    let row = repo::find_active_by_cert_cn(pool, cert_cn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("proyecto '{cert_cn}' no encontrado o ya revocado"))?;
    let ok = repo::revoke(pool, row.id).await.map_err(|e| e.to_string())?;
    if ok {
        println!("revoked {cert_cn} (id={})", row.id);
        Ok(())
    } else {
        Err("revoke no afectó filas".into())
    }
}

async fn cmd_rotate_key(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let cert_cn = args.first().ok_or("uso: rotate-key <cert_cn>")?;
    let row = repo::find_active_by_cert_cn(pool, cert_cn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("proyecto '{cert_cn}' no encontrado"))?;

    let key = api_key::generate();
    sqlx::query!(
        "UPDATE projects
         SET api_key_hash = $2, api_key_salt = $3, api_key_prefix = $4, updated_at = now()
         WHERE id = $1",
        row.id,
        &key.hash.hash[..],
        &key.hash.salt[..],
        key.hash.prefix,
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    println!("rotated key for {cert_cn} (id={})", row.id);
    println!();
    println!("nueva API key (MUÉSTRALA UNA SOLA VEZ):");
    println!("  {}", key.plaintext);
    println!();
    println!("la key vieja sigue siendo válida hasta que expire el cache TTL (30s) — ");
    println!("publica una invalidación a Valkey si necesitas corte inmediato:");
    println!("  redis-cli PUBLISH projects:invalidate {cert_cn}");
    Ok(())
}

fn cmd_admin_hash() -> Result<(), String> {
    let pwd = rpassword::prompt_password("password: ").map_err(|e| e.to_string())?;
    let confirm = rpassword::prompt_password("confirma:  ").map_err(|e| e.to_string())?;
    if pwd != confirm {
        return Err("los passwords no coinciden".into());
    }
    if pwd.len() < 8 {
        return Err("password mínimo 8 chars".into());
    }
    let hash = admin_auth::hash_password(&pwd)?;
    println!();
    println!("ADMIN_PASSWORD_HASH={hash}");
    println!();
    println!("guárdalo como secret en GitHub Actions (o donde manejes secrets)");
    println!("y agrégalo al secret `image-service` del cluster.");
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
