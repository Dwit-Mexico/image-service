// Helper para generar un ADMIN_PASSWORD_HASH local sin tty:
//   cargo run --example gen_admin_hash -- mypassword
//
// Solo para desarrollo. En operación usa `project-admin admin-hash` (lee
// el password desde /dev/tty sin que quede en bash history).

fn main() {
    let arg = std::env::args().nth(1).expect("uso: gen_admin_hash <password>");
    let hash = image_service::admin::auth::hash_password(&arg).expect("hash");
    println!("ADMIN_PASSWORD_HASH={hash}");
}
