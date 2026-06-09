fn main() {
    let hash = std::env::var("ADMIN_PASSWORD_HASH").unwrap();
    println!("hash from env: {hash}");
    let ok = image_service::admin::auth::verify_password("admin1234", &hash);
    println!("verify admin1234: {ok}");
}
