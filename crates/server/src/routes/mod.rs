use axum::routing::{delete, get, patch, post};
use axum::Router;

use crate::state::AppState;

pub mod admin;
pub mod annotations;
pub mod auth;
pub mod billing;
pub mod create;
pub mod feedback;
pub mod golden;
pub mod insights;
pub mod knowledge;
pub mod matters;
pub mod mcp;
pub mod spaces;
pub mod private_vault;
pub mod settings;
pub mod vault;

/// Shared citator context normalization for the Ask-family routes
/// (Ask, research, draft, autonomous): state through the shared
/// vocabulary (unknown codes become None, never a crash), applicant
/// trimmed with empty becoming None. One implementation so every
/// surface normalizes identically.
pub(crate) fn citator_context(
    state: Option<&str>,
    applicant: Option<&str>,
) -> legal_rag_rag::CitatorContext {
    legal_rag_rag::CitatorContext {
        state: state.and_then(legal_rag_rag::citator_v2::normalize_request_state),
        applicant: applicant
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

/// Assemble the full API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(crate::frontend::index))
        .route("/admin", get(crate::frontend::admin_index))
        .route("/admin/", get(crate::frontend::admin_index))
        .route("/admin/{*path}", get(crate::frontend::admin_fallback))
        .route("/health", get(health))
        // Auth wall: local JWT + Auth0-compatible OIDC.
        .route("/api/v1/auth/register", post(auth::register))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/refresh", post(auth::refresh))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/oidc/login", get(auth::oidc_login))
        .route("/api/v1/auth/oidc/callback", get(auth::oidc_callback))
        // Vault: document ingestion (S3 + direct upload).
        .route("/api/v1/vault/search", get(vault::search_documents))
        .route("/api/v1/vault/documents", post(vault::upload_document).get(vault::list_documents))
        .route("/api/v1/vault/documents/{id}", delete(vault::delete_document))
        .route("/api/v1/vault/documents/{id}/chunks", get(vault::document_chunks))
        .route(
            "/api/v1/vault/documents/{id}/metadata",
            patch(vault::update_document_meta),
        )
        .route("/api/v1/vault/ingest", post(vault::ingest_s3))
        // MCP: Model Context Protocol server for external clients.
        .route("/mcp", post(mcp::handle).get(mcp::info))
        .route("/api/v1/admin/mcp", get(mcp::admin_info))
        // Knowledge: RAG query.
        .route("/api/v1/knowledge/query", post(knowledge::query))
        .route("/api/v1/knowledge/feedback", post(feedback::submit_vote))
        .route("/api/v1/admin/feedback", get(feedback::list_votes))
        .route("/api/v1/knowledge/source-document", post(knowledge::source_document))
        .route("/api/v1/knowledge/query/stream", post(knowledge::query_stream))
        .route("/api/v1/knowledge/research", post(knowledge::research))
        .route("/api/v1/knowledge/cache-stats", get(knowledge::cache_stats))
        .route("/api/v1/admin/hit-test", post(knowledge::hit_test))
        .route("/api/v1/admin/retrieval-status", get(knowledge::retrieval_status))
        .route("/api/v1/knowledge/workflows", get(knowledge::workflow_list))
        .route("/api/v1/knowledge/workflows/run", post(knowledge::workflow_run))
        .route("/api/v1/knowledge/agents/autonomous", post(knowledge::autonomous_run))
        .route("/api/v1/knowledge/stats", get(knowledge::stats))
        .route("/api/v1/knowledge/graph", get(knowledge::graph_view))
        // Create: Interact with a document + Draft generation.
        .route("/api/v1/knowledge/interact", post(create::interact))
        .route("/api/v1/knowledge/draft", post(create::draft))
        // Matters: workspaces with living memory.
        .route("/api/v1/matters", get(matters::list_matters).post(matters::create_matter))
        .route(
            "/api/v1/matters/{id}",
            get(matters::get_matter).patch(matters::update_matter).delete(matters::delete_matter),
        )
        .route("/api/v1/matters/{id}/documents", post(matters::attach_document))
        .route("/api/v1/matters/{id}/documents/{doc_id}", delete(matters::detach_document))
        .route("/api/v1/matters/{id}/timeline", get(matters::matter_timeline))
        .route("/api/v1/matters/{id}/memory", get(matters::matter_memory))
        // Spaces: multi-tenant containers above Briefs.
        .route("/api/v1/spaces", get(spaces::list_spaces).post(spaces::create_space))
        .route(
            "/api/v1/spaces/{id}",
            get(spaces::get_space).patch(spaces::update_space).delete(spaces::delete_space),
        )
        .route("/api/v1/spaces/{id}/members", post(spaces::add_member))
        .route("/api/v1/spaces/{id}/members/{user_id}", delete(spaces::remove_member))
        .route("/api/v1/settings/public", get(settings::public_settings))
        .route(
            "/api/v1/admin/settings",
            get(settings::admin_get).put(settings::admin_put),
        )
        .route("/api/v1/private-vault/status", get(private_vault::status))
        .route("/api/v1/private-vault/password", post(private_vault::set_password))
        .route(
            "/api/v1/private-vault/password/change",
            post(private_vault::change_password),
        )
        .route("/api/v1/private-vault/unlock", post(private_vault::unlock))
        .route("/api/v1/private-vault/lock", post(private_vault::lock))
        .route(
            "/api/v1/private-vault/documents",
            get(private_vault::list_documents).post(private_vault::upload_document),
        )
        .route("/api/v1/private-vault/documents/{id}", delete(private_vault::delete_document))
        // Dashboard (user home).
        .route("/api/v1/dashboard/me", get(insights::my_dashboard))
        // Billing: token plans, subscriptions, payments (YouPI-PG UPI).
        .route("/api/v1/billing/plans", get(billing::list_plans))
        .route("/api/v1/billing/me", get(billing::my_billing))
        .route("/api/v1/billing/checkout", post(billing::checkout))
        .route("/api/v1/billing/payments", get(billing::my_payments))
        .route("/api/v1/billing/payments/{id}", get(billing::payment_status))
        .route("/api/v1/billing/payments/{id}/complete-mock", post(billing::complete_mock))
        .route("/api/v1/billing/webhook/youpi", post(billing::youpi_webhook))
        // Command Center: user administration (admin role required).
        .route("/api/v1/admin/citator/backfill", post(admin::backfill_citator))
        .route("/api/v1/admin/citator/v2/backfill", post(admin::backfill_citator_v2))
        .route("/api/v1/admin/users", get(admin::list_users))
        .route("/api/v1/admin/users/{id}/role", post(admin::set_role))
        .route("/api/v1/admin/users/{id}", delete(admin::delete_user))
        .route("/api/v1/admin/billing/overview", get(billing::admin_overview))
        .route("/api/v1/admin/billing/settings", get(billing::admin_get_settings).put(billing::admin_update_settings))
        .route("/api/v1/admin/insights", get(insights::admin_insights))
        .route("/api/v1/admin/insights/users/{id}/activity", get(insights::admin_user_activity))
        // Golden ruleset: evaluation questions + runs.
        .route("/api/v1/admin/golden/rules", get(golden::list_rules).post(golden::add_rule))
        .route("/api/v1/admin/golden/rules/from-activity", post(golden::from_activity))
        .route("/api/v1/admin/golden/rules/{id}", delete(golden::delete_rule).put(golden::update_rule))
        .route("/api/v1/admin/golden/run", post(golden::run_rules))
        .route("/api/v1/admin/golden/stages", post(golden::run_stages))
        // Verified annotations: curated answers served verbatim.
        .route("/api/v1/admin/annotations", get(annotations::list_annotations).post(annotations::create_annotation))
        .route("/api/v1/admin/annotations/from-activity", post(annotations::from_activity))
        .route("/api/v1/admin/annotations/{id}", delete(annotations::delete_annotation))
        .route("/api/v1/admin/annotations/{id}/toggle", post(annotations::toggle_annotation))
        .with_state(state)
        .fallback(crate::frontend::fallback)
}

async fn health() -> &'static str {
    "ok"
}
