use crate::webhooks::github::http_server::handle_github_webhook;
use crate::webhooks::github::security::WebhookSecurity;
use actix_web::{get, middleware, web, App, HttpServer, Responder};
use bitsocial_github_alerts::db::DbPool;
use std::env;

pub async fn run_http_server(pool: DbPool, security: WebhookSecurity) -> std::io::Result<()> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("PORT must be a valid number");

    tracing::info!("Starting HTTP server on port {}", port);

    let pool_data = web::Data::new(pool);
    let security_data = web::Data::new(security);

    HttpServer::new(move || {
        App::new()
            .app_data(pool_data.clone())
            .app_data(security_data.clone())
            .wrap(middleware::Logger::default())
            .service(health)
            .service(handle_github_webhook)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

#[get("/health")]
async fn health() -> impl Responder {
    tracing::info!("Health check");
    "I'm ok"
}
