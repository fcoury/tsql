//! AI-assisted query generation.
//!
//! Translates natural-language prompts into database queries by sending
//! schema-enriched requests to a configurable LLM provider (OpenAI, Anthropic,
//! Google Gemini, OpenRouter, Ollama, or any OpenAI-compatible endpoint).
//!
//! All provider interaction goes through the [`rig`] crate. Each provider has
//! a dedicated `run_*_request` function that builds a client, attaches a system
//! prompt and sampling parameters, and applies a per-request timeout.
//!
//! Responses are expected as `{"query": "...", "explanation": "..."}` JSON.
//! `parse_proposal_response` applies a four-layer fallback (raw JSON, fenced
//! code block, brace extraction, plain text) so the module tolerates models
//! that wrap output in markdown or omit the JSON envelope entirely.
//!
//! Multi-turn conversations are supported: the caller passes prior
//! [`AiTurn`]s, which are formatted into the prompt so the model can refine
//! a query across successive requests.

use std::time::Duration;

use rig::client::{CompletionClient, Nothing};
use rig::completion::Prompt;
use rig::providers::{anthropic, gemini, ollama, openai, openrouter};
use serde::Deserialize;
use tokio::time::timeout;

use crate::config::{AiConfig, AiProvider, DbKind};
use crate::ui::TableInfo;

/// A successfully parsed AI response containing a proposed query and an
/// optional natural-language explanation.
#[derive(Debug, Clone)]
pub struct AiProposal {
    pub query: String,
    pub explanation: Option<String>,
    /// The raw text returned by the provider, kept for debugging.
    pub raw_response: String,
}

/// One user/assistant exchange in a multi-turn AI conversation.
#[derive(Debug, Clone)]
pub struct AiTurn {
    pub user_prompt: String,
    pub assistant_query: String,
}

/// Everything the AI module needs to build a prompt.
///
/// Constructed by `App::start_ai_generation` from the current application
/// state and passed into [`generate_query`].
#[derive(Clone)]
pub struct AiRequestContext {
    pub db_kind: Option<DbKind>,
    pub database_name: Option<String>,
    pub schema_tables: Vec<TableInfo>,
    pub conversation: Vec<AiTurn>,
    pub user_prompt: String,
}

/// Wire format expected from the LLM. Only `query` is required.
#[derive(Debug, Deserialize)]
struct ProposalPayload {
    query: String,
    explanation: Option<String>,
}

/// Send a natural-language prompt to the configured LLM and return a parsed
/// query proposal.
///
/// Builds a system prompt (engine-aware, optionally user-overridden) and a
/// user prompt containing schema context and conversation history, dispatches
/// to the appropriate provider, and parses the response.
///
/// # Errors
///
/// Returns a human-readable `String` on client initialization failure,
/// request timeout, provider HTTP error, or unparseable response.
pub async fn generate_query(
    config: &AiConfig,
    context: &AiRequestContext,
) -> std::result::Result<AiProposal, String> {
    let system_prompt = build_system_prompt(config, context.db_kind);
    let prompt = build_prompt(config, context);

    let response = match config.provider {
        AiProvider::OpenAi | AiProvider::OpenAiCompatible => {
            run_openai_request(config, &system_prompt, &prompt).await
        }
        AiProvider::Ollama => run_ollama_request(config, &system_prompt, &prompt).await,
        AiProvider::Anthropic => run_anthropic_request(config, &system_prompt, &prompt).await,
        AiProvider::Google => run_google_request(config, &system_prompt, &prompt).await,
        AiProvider::OpenRouter => run_openrouter_request(config, &system_prompt, &prompt).await,
    }?;

    let (query, explanation) = parse_proposal_response(&response)?;

    Ok(AiProposal {
        query,
        explanation,
        raw_response: response,
    })
}

async fn run_openai_request(
    config: &AiConfig,
    system_prompt: &str,
    prompt: &str,
) -> std::result::Result<String, String> {
    if config.provider == AiProvider::OpenAiCompatible
        && config.base_url.as_deref().unwrap_or("").trim().is_empty()
    {
        return Err(
            "AI config error: `ai.base_url` is required for `open_ai_compatible`".to_string(),
        );
    }

    let api_key = resolve_api_key(config, config.provider == AiProvider::OpenAiCompatible)?;

    let mut builder: openai::ClientBuilder = openai::Client::builder().api_key(&api_key);
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.base_url(base_url);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to initialize OpenAI client: {e}"))?;

    let agent = client
        .agent(config.model.as_str())
        .preamble(system_prompt)
        .temperature(config.temperature)
        .max_tokens(config.max_tokens)
        .build();

    let response = timeout(
        Duration::from_secs(config.request_timeout_secs),
        agent.prompt(prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "AI request timed out after {}s",
            config.request_timeout_secs
        )
    })?
    .map_err(|e| format!("AI provider error: {e}"))?;

    Ok(response)
}

async fn run_anthropic_request(
    config: &AiConfig,
    system_prompt: &str,
    prompt: &str,
) -> std::result::Result<String, String> {
    let api_key = resolve_api_key(config, false)?;

    let mut builder: anthropic::ClientBuilder = anthropic::Client::builder().api_key(api_key);
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.base_url(base_url);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to initialize Anthropic client: {e}"))?;

    let agent = client
        .agent(config.model.as_str())
        .preamble(system_prompt)
        .temperature(config.temperature)
        .max_tokens(config.max_tokens)
        .build();

    let response = timeout(
        Duration::from_secs(config.request_timeout_secs),
        agent.prompt(prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "AI request timed out after {}s",
            config.request_timeout_secs
        )
    })?
    .map_err(|e| format!("AI provider error: {e}"))?;

    Ok(response)
}

async fn run_google_request(
    config: &AiConfig,
    system_prompt: &str,
    prompt: &str,
) -> std::result::Result<String, String> {
    let api_key = resolve_api_key(config, false)?;

    let mut builder = gemini::Client::builder().api_key(api_key);
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.base_url(base_url);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to initialize Google Gemini client: {e}"))?;

    let agent = client
        .agent(config.model.as_str())
        .preamble(system_prompt)
        .temperature(config.temperature)
        .max_tokens(config.max_tokens)
        .build();

    let response = timeout(
        Duration::from_secs(config.request_timeout_secs),
        agent.prompt(prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "AI request timed out after {}s",
            config.request_timeout_secs
        )
    })?
    .map_err(|e| format!("AI provider error: {e}"))?;

    Ok(response)
}

async fn run_openrouter_request(
    config: &AiConfig,
    system_prompt: &str,
    prompt: &str,
) -> std::result::Result<String, String> {
    let api_key = resolve_api_key(config, false)?;

    let mut builder: openrouter::ClientBuilder = openrouter::Client::builder().api_key(api_key);
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.base_url(base_url);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to initialize OpenRouter client: {e}"))?;

    let agent = client
        .agent(config.model.as_str())
        .preamble(system_prompt)
        .temperature(config.temperature)
        .max_tokens(config.max_tokens)
        .build();

    let response = timeout(
        Duration::from_secs(config.request_timeout_secs),
        agent.prompt(prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "AI request timed out after {}s",
            config.request_timeout_secs
        )
    })?
    .map_err(|e| format!("AI provider error: {e}"))?;

    Ok(response)
}

async fn run_ollama_request(
    config: &AiConfig,
    system_prompt: &str,
    prompt: &str,
) -> std::result::Result<String, String> {
    let mut builder: ollama::ClientBuilder = ollama::Client::builder().api_key(Nothing);
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.base_url(base_url);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to initialize Ollama client: {e}"))?;

    let agent = client
        .agent(config.model.as_str())
        .preamble(system_prompt)
        .temperature(config.temperature)
        .max_tokens(config.max_tokens)
        .build();

    let response = timeout(
        Duration::from_secs(config.request_timeout_secs),
        agent.prompt(prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "AI request timed out after {}s",
            config.request_timeout_secs
        )
    })?
    .map_err(|e| format!("AI provider error: {e}"))?;

    Ok(response)
}

/// Read the API key from the environment.
///
/// Tries each candidate env var in order (see [`api_key_env_candidates`]).
/// When `allow_missing` is true (used for OpenAI-compatible endpoints that
/// may not need auth), returns a dummy value instead of erroring.
fn resolve_api_key(config: &AiConfig, allow_missing: bool) -> std::result::Result<String, String> {
    let candidates = api_key_env_candidates(config);
    for env_var in &candidates {
        if let Ok(value) = std::env::var(env_var) {
            if !value.trim().is_empty() {
                return Ok(value);
            }
        }
    }

    if allow_missing {
        return Ok("dummy".to_string());
    }

    if candidates.is_empty() {
        return Err("AI config error: `ai.api_key_env` is empty".to_string());
    }

    let formatted = candidates
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "AI config error: none of env vars [{formatted}] are set (or empty)"
    ))
}

/// Determine which environment variable names to check for the API key.
///
/// If the user left the default `OPENAI_API_KEY` but selected a non-OpenAI
/// provider, the provider's conventional env var is tried first so a user
/// who only sets `ANTHROPIC_API_KEY` does not also need to override
/// `api_key_env` in config. An explicitly customized value is used as-is.
fn api_key_env_candidates(config: &AiConfig) -> Vec<String> {
    let configured = config.api_key_env.trim();
    let provider_default = default_api_key_env_for_provider(config.provider);

    if configured.is_empty() {
        return provider_default
            .map(|name| vec![name.to_string()])
            .unwrap_or_default();
    }

    if configured == "OPENAI_API_KEY" {
        if let Some(default_env) = provider_default {
            if default_env != "OPENAI_API_KEY" {
                return vec![default_env.to_string(), configured.to_string()];
            }
        }
    }

    vec![configured.to_string()]
}

fn default_api_key_env_for_provider(provider: AiProvider) -> Option<&'static str> {
    match provider {
        AiProvider::OpenAi | AiProvider::OpenAiCompatible => Some("OPENAI_API_KEY"),
        AiProvider::Anthropic => Some("ANTHROPIC_API_KEY"),
        AiProvider::Google => Some("GEMINI_API_KEY"),
        AiProvider::OpenRouter => Some("OPENROUTER_API_KEY"),
        AiProvider::Ollama => None,
    }
}

fn build_system_prompt(config: &AiConfig, db_kind: Option<DbKind>) -> String {
    let (default_prompt, override_prompt) = match db_kind {
        Some(DbKind::Mongo) => (
            "You are a MongoDB query assistant for tsql.
Return a query that tsql can execute:
- Prefer `db.<collection>.<operation>(...)` syntax.
- You may also use JSON command syntax: {\"op\":\"find\",...}.
- Use only operations supported by tsql: find, findOne, aggregate, countDocuments, insertOne, insertMany, updateOne, updateMany, deleteOne, deleteMany.
- Do not output markdown fences.
- Keep response concise.",
            config.system_prompt_mongo.as_deref(),
        ),
        _ => (
            "You are a PostgreSQL query assistant for tsql.
Return a single PostgreSQL query or statement block.
- Do not output markdown fences.
- Keep response concise.
- If request is ambiguous, choose a safe query and explain assumptions briefly.",
            config.system_prompt_postgres.as_deref(),
        ),
    };

    override_prompt.unwrap_or(default_prompt).to_string()
}

/// Assemble the user-facing prompt from engine type, database name, schema
/// summary, conversation history, and the current user request. Terminates
/// with a JSON format instruction so the model returns a structured payload.
fn build_prompt(config: &AiConfig, context: &AiRequestContext) -> String {
    let mut sections = Vec::new();

    let engine = match context.db_kind {
        Some(DbKind::Mongo) => "mongo",
        _ => "postgres",
    };
    sections.push(format!("engine: {engine}"));

    if let Some(name) = context.database_name.as_deref().filter(|s| !s.is_empty()) {
        sections.push(format!("database: {name}"));
    }

    if config.include_schema_context {
        let schema_summary = summarize_schema(
            &context.schema_tables,
            config.max_schema_tables,
            config.max_columns_per_table,
        );
        if !schema_summary.is_empty() {
            sections.push(format!("schema:\n{schema_summary}"));
        }
    }

    if !context.conversation.is_empty() {
        let mut history = String::from("conversation:");
        for (idx, turn) in context.conversation.iter().enumerate() {
            history.push_str(&format!(
                "\n{}. user: {}\n{}. assistant_query: {}",
                idx + 1,
                turn.user_prompt.trim(),
                idx + 1,
                turn.assistant_query.trim()
            ));
        }
        sections.push(history);
    }

    sections.push(format!("request:\n{}", context.user_prompt.trim()));
    sections.push(
        "Return strictly JSON object with keys:\n{\"query\": \"...\", \"explanation\": \"...\"}\n`query` is required."
            .to_string(),
    );

    sections.join("\n\n")
}

fn summarize_schema(tables: &[TableInfo], max_tables: usize, max_columns: usize) -> String {
    if tables.is_empty() || max_tables == 0 || max_columns == 0 {
        return String::new();
    }

    let mut lines = Vec::new();
    for table in tables.iter().take(max_tables) {
        let mut cols = Vec::new();
        for col in table.columns.iter().take(max_columns) {
            cols.push(col.name.clone());
        }
        if table.columns.len() > max_columns {
            cols.push("...".to_string());
        }
        lines.push(format!(
            "- {}.{}({})",
            table.schema,
            table.name,
            cols.join(", ")
        ));
    }
    if tables.len() > max_tables {
        lines.push(format!("- ... {} more", tables.len() - max_tables));
    }

    lines.join("\n")
}

/// Extract a `(query, explanation)` pair from a raw LLM response.
///
/// Applies four parsing strategies in order:
/// 1. Direct JSON deserialization of the entire response.
/// 2. Extract JSON from a markdown fenced code block (` ```json ... ``` `).
/// 3. Extract the first `{...}` substring and try JSON deserialization.
/// 4. Strip markdown fences and treat the remainder as a plain-text query.
///
/// Returns an error only if every strategy yields an empty or absent query.
fn parse_proposal_response(raw: &str) -> std::result::Result<(String, Option<String>), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("AI returned an empty response".to_string());
    }

    if let Ok(payload) = serde_json::from_str::<ProposalPayload>(trimmed) {
        return validate_payload(payload);
    }

    if let Some(json_candidate) = extract_json_code_block(trimmed) {
        if let Ok(payload) = serde_json::from_str::<ProposalPayload>(&json_candidate) {
            return validate_payload(payload);
        }
    }

    if let Some(braced) = extract_first_json_object(trimmed) {
        if let Ok(payload) = serde_json::from_str::<ProposalPayload>(&braced) {
            return validate_payload(payload);
        }
    }

    let fallback_query = strip_markdown_fences(trimmed).trim().to_string();
    if fallback_query.is_empty() {
        return Err("AI response did not include a usable query".to_string());
    }
    Ok((fallback_query, None))
}

fn validate_payload(
    payload: ProposalPayload,
) -> std::result::Result<(String, Option<String>), String> {
    let query = payload.query.trim().to_string();
    if query.is_empty() {
        return Err("AI response JSON contained an empty `query`".to_string());
    }
    Ok((query, payload.explanation.map(|s| s.trim().to_string())))
}

fn extract_json_code_block(input: &str) -> Option<String> {
    let mut start = input.find("```")?;
    loop {
        let after_ticks = &input[start + 3..];
        let end_rel = after_ticks.find("```")?;
        let block = &after_ticks[..end_rel];
        let normalized = block
            .strip_prefix("json")
            .or_else(|| block.strip_prefix("JSON"))
            .map(str::trim_start)
            .unwrap_or(block)
            .trim();
        if normalized.starts_with('{') {
            return Some(normalized.to_string());
        }
        let next = &after_ticks[end_rel + 3..];
        let offset = next.find("```")?;
        start = input.len() - next.len() + offset;
    }
}

fn extract_first_json_object(input: &str) -> Option<String> {
    let start = input.find('{')?;
    let end = input.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(input[start..=end].to_string())
}

fn strip_markdown_fences(input: &str) -> String {
    let mut out = input.trim().to_string();
    if out.starts_with("```") {
        out = out
            .trim_start_matches("```json")
            .trim_start_matches("```JSON")
            .trim_start_matches("```")
            .to_string();
    }
    if out.ends_with("```") {
        out = out.trim_end_matches("```").to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_from_json() {
        let raw = r#"{"query":"select 1","explanation":"test"}"#;
        let (query, explanation) = parse_proposal_response(raw).unwrap();
        assert_eq!(query, "select 1");
        assert_eq!(explanation.as_deref(), Some("test"));
    }

    #[test]
    fn parse_payload_from_json_fence() {
        let raw = "```json\n{\"query\":\"select * from users\"}\n```";
        let (query, explanation) = parse_proposal_response(raw).unwrap();
        assert_eq!(query, "select * from users");
        assert!(explanation.is_none());
    }

    #[test]
    fn parse_payload_fallback_plain_text() {
        let raw = "db.users.find({\"active\": true})";
        let (query, explanation) = parse_proposal_response(raw).unwrap();
        assert_eq!(query, raw);
        assert!(explanation.is_none());
    }

    #[test]
    fn parse_payload_rejects_empty_query() {
        let raw = "{\"query\":\"   \"}";
        let err = parse_proposal_response(raw).unwrap_err();
        assert!(err.contains("empty `query`"));
    }

    #[test]
    fn api_key_env_candidates_fallback_to_provider_default_when_openai_default_is_implicit() {
        let config = AiConfig {
            provider: AiProvider::OpenRouter,
            api_key_env: "OPENAI_API_KEY".to_string(),
            ..AiConfig::default()
        };
        assert_eq!(
            api_key_env_candidates(&config),
            vec![
                "OPENROUTER_API_KEY".to_string(),
                "OPENAI_API_KEY".to_string()
            ]
        );
    }

    #[test]
    fn api_key_env_candidates_keep_explicit_custom_value() {
        let config = AiConfig {
            provider: AiProvider::OpenRouter,
            api_key_env: "MY_OPENROUTER_TOKEN".to_string(),
            ..AiConfig::default()
        };
        assert_eq!(
            api_key_env_candidates(&config),
            vec!["MY_OPENROUTER_TOKEN".to_string()]
        );
    }
}
