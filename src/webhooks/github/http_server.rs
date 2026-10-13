use crate::utils::branch_filter::BranchFilter;
use crate::webhooks::common::{process_webhook, WebhookContext};
use crate::webhooks::github::dependabot_filter::{
    has_skip_notification_marker, is_dependabot_event,
};
use crate::webhooks::github::security::{WebhookRejection, WebhookSecurity};
use crate::webhooks::github::webhook_handlers::{
    handle_check_run_event, handle_comment_event, handle_create_event, handle_delete_event,
    handle_issue_event, handle_ping_event, handle_pull_request_event, handle_push_event,
    handle_release_event, handle_wiki_event, handle_workflow_run_event,
};
use actix_web::{post, web, HttpRequest, HttpResponse, Responder};
use bitsocial_github_alerts::db::DbPool;
use serde::Deserialize;
use std::env;

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    pub branch: Option<String>,
    pub exclude_branch: Option<String>,
}

#[post("/github/{webhook_url}")]
pub async fn handle_github_webhook(
    pool: web::Data<DbPool>,
    security: web::Data<WebhookSecurity>,
    webhook_url: web::Path<String>,
    query: web::Query<QueryParams>,
    req: HttpRequest,
    body: web::Bytes,
) -> impl Responder {
    // Reject unauthenticated or foreign deliveries before filters, payload logs, DB or Telegram.
    if let Err(rejection) = security.authorize(req.headers(), &body) {
        if rejection == WebhookRejection::NonPublicRepository {
            tracing::info!(
                "Ignoring GitHub webhook for a non-public or unknown-visibility repository"
            );
            return HttpResponse::Ok().finish();
        }
        tracing::warn!("Rejected GitHub webhook: {:?}", rejection);
        return match rejection {
            WebhookRejection::InvalidSignature => HttpResponse::Unauthorized().finish(),
            WebhookRejection::DisallowedOwner => HttpResponse::Forbidden().finish(),
            WebhookRejection::InvalidEvent | WebhookRejection::InvalidPayload => {
                HttpResponse::BadRequest().finish()
            }
            WebhookRejection::NonPublicRepository => unreachable!("handled above"),
        };
    }

    if let Some(event_name) = req.headers().get("x-github-event") {
        tracing::info!("Event name: {:?}", event_name);

        let branch_filter =
            match BranchFilter::new(query.branch.as_deref(), query.exclude_branch.as_deref()) {
                Ok(filter) => Some(filter),
                Err(e) => {
                    tracing::error!("Invalid branch filter pattern: {}", e);
                    return HttpResponse::BadRequest().finish();
                }
            };

        let event_str = event_name.to_str().unwrap_or("unknown");

        if is_dependabot_event(&body) {
            tracing::info!("Suppressing Dependabot webhook event: {}", event_str);
            return HttpResponse::Ok().finish();
        }

        if event_str == "push" && has_skip_notification_marker(&body) {
            tracing::info!("Suppressing GitHub push event with notification skip marker");
            return HttpResponse::Ok().finish();
        }

        let message = match event_str {
            "ping" => handle_ping_event(&body),
            "push" => handle_push_event(&body, branch_filter.as_ref()),
            "issues" => handle_issue_event(&body),
            "pull_request" => handle_pull_request_event(&body, branch_filter.as_ref()),
            "issue_comment" | "pull_request_review_comment" | "commit_comment" => {
                handle_comment_event(&body, false)
            }
            "check_run" => handle_check_run_event(&body),
            "create" => handle_create_event(&body, branch_filter.as_ref()),
            "delete" => handle_delete_event(&body, branch_filter.as_ref()),
            "gollum" => handle_wiki_event(&body),
            "release" => handle_release_event(&body),
            "workflow_run" => handle_workflow_run_event(&body, branch_filter.as_ref()),
            _ => String::new(),
        };
        tracing::info!("Message: {}", message);

        let github_token = match env::var("GITHUB_TELOXIDE_TOKEN") {
            Ok(token) => token,
            Err(_) => {
                tracing::error!("GITHUB_TELOXIDE_TOKEN not set");
                return HttpResponse::InternalServerError().finish();
            }
        };

        process_webhook(WebhookContext {
            pool: pool.get_ref(),
            webhook_url: &webhook_url,
            message,
            bot_name: "Github",
            token: github_token,
            event_name: event_str,
            source: "github",
            link_preview: event_str == "release",
        })
        .await
    } else {
        HttpResponse::BadRequest().finish()
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};
    use diesel::{r2d2::ConnectionManager, PgConnection};
    use hmac::{Hmac, KeyInit, Mac};
    use serde_json::json;
    use sha2::Sha256;
    use std::sync::Arc;

    const SECRET: &str = "integration-test-secret-at-least-32-bytes";

    #[actix_web::test]
    async fn enforces_security_before_suppression_database_or_telegram() {
        // No DB or Telegram credentials are available: accepted probes must stop at
        // suppression, and rejected probes must stop before reaching either service.
        let pool = diesel::r2d2::Pool::builder()
            .min_idle(Some(0))
            .build_unchecked(ConnectionManager::<PgConnection>::new(
                "postgresql://localhost:1/unused",
            ));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Arc::new(pool)))
                .app_data(web::Data::new(WebhookSecurity::new(SECRET).unwrap()))
                .service(handle_github_webhook),
        )
        .await;

        for (owner, signed, expected) in [
            (243813100_u64, false, StatusCode::UNAUTHORIZED),
            (1, false, StatusCode::UNAUTHORIZED),
            (1, true, StatusCode::FORBIDDEN),
            (243813100, true, StatusCode::OK),
            (296465714, true, StatusCode::OK),
            (245761493, true, StatusCode::OK),
        ] {
            let json = json!({
                "repository": {"private": false, "owner": {"id": owner, "type": "Organization"}},
                "head_commit": {"message": "[skip github-alerts] security test"}
            })
            .to_string();
            let form = serde_urlencoded::to_string([("payload", &json)]).unwrap();
            for (body, content_type) in [
                (&json, "application/json"),
                (&form, "application/x-www-form-urlencoded"),
            ] {
                let mut request = test::TestRequest::post()
                    .uri("/github/test-destination")
                    .insert_header(("content-type", content_type))
                    .insert_header(("x-github-event", "push"))
                    .set_payload(body.clone());
                if signed {
                    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
                    mac.update(body.as_bytes());
                    request = request.insert_header((
                        "x-hub-signature-256",
                        format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
                    ));
                }
                let response = test::call_service(&app, request.to_request()).await;
                assert_eq!(
                    response.status(),
                    expected,
                    "owner={owner}, signed={signed}, content_type={content_type}"
                );
            }
        }

        for event in ["ping", "issues", "unknown"] {
            let request = test::TestRequest::post()
                .uri("/github/test-destination")
                .insert_header(("x-github-event", event))
                .set_payload(r#"{"sender":{"login":"dependabot[bot]"}}"#)
                .to_request();
            let response = test::call_service(&app, request).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        // These have no Dependabot sender or skip marker. A 200 response proves
        // excluded content cannot reach formatting, database access or Telegram.
        for owner in [243813100_u64, 296465714, 245761493, 1] {
            for private in [
                None,
                Some(json!(true)),
                Some(json!(null)),
                Some(json!("false")),
            ] {
                let mut payload = json!({
                    "repository": {"owner": {"id": owner, "type": "Organization"}},
                    "action": "opened",
                    "issue": {"title": "private content must never be processed"}
                });
                if let Some(value) = private {
                    payload["repository"]["private"] = value;
                }
                let json = payload.to_string();
                let form = serde_urlencoded::to_string([("payload", &json)]).unwrap();
                for event in ["push", "issues", "release", "ping"] {
                    for (body, content_type) in [
                        (&json, "application/json"),
                        (&form, "application/x-www-form-urlencoded"),
                    ] {
                        for signed in [false, true] {
                            let mut request = test::TestRequest::post()
                                .uri("/github/test-destination")
                                .insert_header(("content-type", content_type))
                                .insert_header(("x-github-event", event))
                                .set_payload(body.clone());
                            if signed {
                                let mut mac =
                                    Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
                                mac.update(body.as_bytes());
                                request = request.insert_header((
                                    "x-hub-signature-256",
                                    format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
                                ));
                            }
                            let expected = if !signed {
                                StatusCode::UNAUTHORIZED
                            } else if owner == 1 {
                                StatusCode::FORBIDDEN
                            } else {
                                StatusCode::OK
                            };
                            let response = test::call_service(&app, request.to_request()).await;
                            assert_eq!(
                                response.status(),
                                expected,
                                "owner={owner}, signed={signed}, event={event}"
                            );
                        }
                    }
                }
            }
        }
    }
}
