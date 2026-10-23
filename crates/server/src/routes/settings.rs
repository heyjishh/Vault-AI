//! Customization endpoints: public panel settings for the user app, and
//! the admin Command Center's read/write access to all of them.

use axum::extract::State;
use axum::Json;
use legal_rag_auth::AdminUser;
use legal_rag_common::Error as DomainError;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::settings::Settings;
use crate::state::AppState;

/// Push the saved prompt overrides into every live surface. Interact and
/// Draft read settings per request, so they need no push.
pub fn apply_prompts(state: &AppState, settings: &Settings) {
    state.query.set_answer_system(settings.prompts.ask.clone());
    state.query.set_research_prompts(
        settings.prompts.research_frame.clone(),
        settings.prompts.research_adjudicate.clone(),
    );
    state.workflows.set_instruction(settings.prompts.agent.clone());
    state.agent.set_instruction(settings.prompts.agent.clone());
}

/// What the user panel needs: branding, visible custom tabs, and the
/// default answer mode. No auth — the login screen itself is branded.
pub async fn public_settings(State(state): State<AppState>) -> Json<Value> {
    let s = state.settings.get();
    Json(json!({
        "brand": s.brand,
        "default_mode": s.default_mode,
        "tabs": s.tabs.iter().filter(|t| t.visible).cloned().collect::<Vec<_>>(),
    }))
}

/// Full settings + what is actually in effect right now + the built-in
/// default prompts (so the panel can prefill editors with the real text).
pub async fn admin_get(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<Value> {
    let s = state.settings.get();
    let defaults = json!({
        "ask": legal_rag_rag::query::ANSWER_SYSTEM,
        "interact": crate::routes::create::ANALYST_SYSTEM,
        "draft": crate::routes::create::DRAFTER_SYSTEM,
        "research_frame": legal_rag_rag::research::FRAME_SYSTEM,
        "research_adjudicate": legal_rag_rag::research::ADJUDICATE_SYSTEM,
    });
    let agent_workflows: Vec<Value> = state
        .workflows
        .list()
        .iter()
        .map(|w| json!({ "id": w.id, "name": w.name, "report_instruction": w.report_instruction }))
        .collect();
    Json(json!({
        "settings": s,
        "defaults": defaults,
        "agent_workflows": agent_workflows,
        "effective": {
            "llm_model": state.dyn_llm.as_ref().map(|d| d.model()).unwrap_or_else(|| state.default_llm_model.clone()),
            "embedding_model": state.boot_embedding_model,
            "default_mode": s.default_mode,
            "prompts_active": {
                "ask": s.prompts.ask.is_some(),
                "interact": s.prompts.interact.is_some(),
                "draft": s.prompts.draft.is_some(),
                "research_frame": s.prompts.research_frame.is_some(),
                "research_adjudicate": s.prompts.research_adjudicate.is_some(),
                "agent": s.prompts.agent.is_some(),
            },
        },
    }))
}

/// Save settings and apply everything that can change live: the default
/// mode and branding/tabs are read per request; the system prompt and
/// the LLM model swap immediately. The embedding model is persisted and
/// takes effect at the next restart (vectors are not interchangeable).
pub async fn admin_put(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Json(new_settings): Json<Settings>,
) -> Result<Json<Value>, ApiError> {
    let saved = state
        .settings
        .update(new_settings)
        .map_err(|e| ApiError(DomainError::BadRequest(e)))?;
    apply_prompts(&state, &saved);
    if let Some(dyn_llm) = &state.dyn_llm {
        let target = saved.llm_model.clone().unwrap_or_else(|| state.default_llm_model.clone());
        if dyn_llm.model() != target {
            dyn_llm.set_model(&target).map_err(ApiError::from)?;
        }
    }
    Ok(Json(json!({ "ok": true, "settings": saved })))
}
