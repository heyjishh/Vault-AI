//! `/mcp` — Model Context Protocol server (streamable-HTTP style,
//! single POST endpoint, JSON-RPC 2.0). Exposes the vault to external
//! MCP clients (Claude, IDEs, other agents) through three tools:
//! `search_vault` (retrieval only), `list_documents`, and `ask`
//! (grounded answer with citations).
//!
//! Auth: the caller's normal bearer token (AuthUser) — an MCP client
//! connects as a user and inherits exactly that user's access, quota
//! included. Disable with LEGAL_RAG_MCP=0.

use axum::extract::State;
use axum::Json;
use legal_rag_auth::{AdminUser, AuthUser};
use legal_rag_rag::QueryMode;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

pub fn mcp_enabled() -> bool {
    std::env::var("LEGAL_RAG_MCP").map(|v| v != "0").unwrap_or(true)
}

const TOOLS: &[(&str, &str)] = &[
    (
        "search_vault",
        "Search the Juris Vault AI corpus and return the top matching passages (chunk id, document, score, text). Retrieval only — no generated answer.",
    ),
    (
        "list_documents",
        "List the documents in the vault (id, title, chunk count).",
    ),
    (
        "ask",
        "Ask a legal question and get a grounded answer with citations from the vault corpus.",
    ),
];

fn tool_defs() -> Value {
    json!([
        {
            "name": "search_vault",
            "description": TOOLS[0].1,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 20 }
                },
                "required": ["query"]
            }
        },
        {
            "name": "list_documents",
            "description": TOOLS[1].1,
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "ask",
            "description": TOOLS[2].1,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 20 }
                },
                "required": ["question"]
            }
        }
    ])
}

#[derive(Deserialize)]
pub struct RpcRequest {
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

fn rpc_ok(id: &Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "result": result })
}

fn rpc_err(id: &Option<Value>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.clone().unwrap_or(Value::Null),
        "error": { "code": code, "message": message }
    })
}

fn tool_text(payload: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "isError": false
    })
}

/// POST /mcp — one JSON-RPC request per call.
pub async fn handle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<RpcRequest>,
) -> Result<Json<Value>, ApiError> {
    if !mcp_enabled() {
        return Ok(Json(rpc_err(&req.id, -32000, "MCP server is disabled")));
    }
    let id = req.id.clone();
    let result = match req.method.as_str() {
        "initialize" => Ok(rpc_ok(
            &id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "juris-vault-ai", "version": "1.0.0" }
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => {
            Ok(rpc_ok(&id, json!({})))
        }
        "ping" => Ok(rpc_ok(&id, json!({}))),
        "tools/list" => Ok(rpc_ok(&id, json!({ "tools": tool_defs() }))),
        "tools/call" => {
            let name = req.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(&state, &user.id, name, &args).await {
                Ok(payload) => Ok(rpc_ok(&id, tool_text(payload))),
                Err(msg) => Ok(rpc_err(&id, -32602, &msg)),
            }
        }
        other => Ok(rpc_err(&id, -32601, &format!("method not found: {other}"))),
    };
    result.map(Json)
}

async fn call_tool(
    state: &AppState,
    user_id: &str,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    match name {
        "search_vault" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            if query.trim().is_empty() {
                return Err("query is empty".to_string());
            }
            let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let report = state
                .query
                .hit_test(query.trim(), top_k.clamp(1, 20))
                .await
                .map_err(|e| e.to_string())?;
            let hits = report
                .stages
                .last()
                .map(|s| {
                    s.hits
                        .iter()
                        .map(|h| {
                            json!({
                                "chunk_id": h.chunk_id,
                                "doc_id": h.doc_id,
                                "doc_title": h.doc_title,
                                "score": h.score,
                                "text": h.snippet,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(json!({ "query": query, "hits": hits }))
        }
        "list_documents" => {
            let docs = state
                .rag
                .list_documents()
                .into_iter()
                .map(|d| json!({ "id": d.id, "title": d.title, "chunks": d.chunks }))
                .collect::<Vec<_>>();
            Ok(json!({ "documents": docs }))
        }
        "ask" => {
            let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
            if question.trim().is_empty() {
                return Err("question is empty".to_string());
            }
            crate::routes::billing::ensure_quota(state, user_id)
                .map_err(|e| e.0.to_string())?;
            let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let answer = state
                .query
                .query(question.trim(), QueryMode::Mix, top_k.clamp(1, 20), legal_rag_rag::CitatorContext::default())
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(&answer).unwrap_or(json!({})))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// GET /mcp — connection info for humans and the admin panel.
pub async fn info() -> Json<Value> {
    Json(json!({
        "name": "juris-vault-ai",
        "transport": "streamable-http (single POST endpoint, JSON-RPC 2.0)",
        "endpoint": "/mcp",
        "enabled": mcp_enabled(),
        "auth": "Bearer token (the user's normal API token)",
        "tools": TOOLS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect::<Vec<_>>(),
    }))
}

/// GET /api/v1/admin/mcp — admin view of the MCP surface.
pub async fn admin_info(
    State(_state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<Value> {
    Json(json!({
        "enabled": mcp_enabled(),
        "endpoint": "/mcp",
        "transport": "streamable-http",
        "protocol_version": "2024-11-05",
        "tools": TOOLS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect::<Vec<_>>(),
    }))
}
