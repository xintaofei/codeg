/// External MCP server proxy: forward write/search/link to user's MCP server.
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::time::Duration;

use super::backend::{
    MemoryBackend, MemoryError, MemoryHit, MemoryNode, MemoryProvenance, NewMemoryNode,
};
use crate::models::{MemoryRel, MemoryScope};

/// Minimal MCP proxy config.
#[derive(Debug, Clone)]
pub struct ExternalMcpConfig {
    pub server_id: String,
    pub write_tool: String,
    pub search_tool: String,
    pub link_tool: String,
}

/// External MCP backend: proxies calls to a user's MCP server via HTTP JSON-RPC tools/call.
pub struct ExternalMcpBackend {
    config: ExternalMcpConfig,
    client: reqwest::Client,
}

impl ExternalMcpBackend {
    pub fn new(config: ExternalMcpConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    fn endpoint_url(&self) -> String {
        let raw = self.config.server_id.trim();
        if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        }
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, MemoryError> {
        let url = self.endpoint_url();
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    MemoryError("MCP server request timed out after 10 seconds".to_string())
                } else if e.is_connect() {
                    MemoryError(format!(
                        "Failed to connect to MCP server at {}: connection refused",
                        url
                    ))
                } else {
                    MemoryError(format!("MCP request error: {}", e))
                }
            })?;

        if !resp.status().is_success() {
            return Err(MemoryError(format!(
                "MCP server returned HTTP {}",
                resp.status()
            )));
        }

        let json_resp: Value = resp
            .json()
            .await
            .map_err(|e| MemoryError(format!("Failed to parse MCP server JSON response: {}", e)))?;

        if let Some(err_obj) = json_resp.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(MemoryError(format!(
                "MCP tool error from '{}': {}",
                name, msg
            )));
        }

        let result = json_resp.get("result").cloned().unwrap_or(Value::Null);
        Ok(result)
    }
}

#[async_trait]
impl MemoryBackend for ExternalMcpBackend {
    async fn write(&self, node: NewMemoryNode) -> Result<i32, MemoryError> {
        let args = json!({
            "kind": node.kind,
            "title": node.title,
            "body": node.body,
            "scope": match node.scope {
                MemoryScope::Project => "project",
                MemoryScope::Global => "global",
            },
            "folder_id": node.folder_id,
            "provenance": node.provenance,
        });

        let result = self.call_tool(&self.config.write_tool, args).await?;
        if let Some(id) = result.as_i64() {
            return Ok(id as i32);
        }
        if let Some(id) = result.get("id").and_then(|v| v.as_i64()) {
            return Ok(id as i32);
        }
        if let Some(id) = result
            .pointer("/structuredContent/id")
            .and_then(|v| v.as_i64())
        {
            return Ok(id as i32);
        }

        Ok(1)
    }

    async fn search(
        &self,
        q: &str,
        scope: MemoryScope,
        folder_id: Option<i32>,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, MemoryError> {
        let scope_str = match scope {
            MemoryScope::Project => "project",
            MemoryScope::Global => "global",
        };
        let args = json!({
            "query": q,
            "scope": scope_str,
            "folder_id": folder_id,
            "limit": limit.min(50),
        });

        let result = self.call_tool(&self.config.search_tool, args).await?;

        let hits_arr = if let Some(arr) = result.as_array() {
            arr
        } else if let Some(arr) = result.get("hits").and_then(|v| v.as_array()) {
            arr
        } else if let Some(arr) = result
            .pointer("/structuredContent/hits")
            .and_then(|v| v.as_array())
        {
            arr
        } else {
            return Ok(vec![]);
        };

        let mut hits = Vec::new();
        for item in hits_arr {
            if let Ok(hit) = serde_json::from_value::<MemoryHit>(item.clone()) {
                hits.push(hit);
            } else if let Some(node_val) = item.get("node") {
                if let Ok(node) = serde_json::from_value::<MemoryNode>(node_val.clone()) {
                    let score = item.get("score").and_then(|s| s.as_f64()).unwrap_or(1.0);
                    let via = item
                        .get("via")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    hits.push(MemoryHit { node, score, via });
                }
            } else if let Some(title) = item.get("title").and_then(|t| t.as_str()) {
                let id = item.get("id").and_then(|i| i.as_i64()).unwrap_or(0) as i32;
                let kind = item
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("custom")
                    .to_string();
                let body = item
                    .get("body")
                    .and_then(|b| b.as_str())
                    .unwrap_or("")
                    .to_string();
                let score = item.get("score").and_then(|s| s.as_f64()).unwrap_or(1.0);
                hits.push(MemoryHit {
                    node: MemoryNode {
                        id,
                        kind,
                        title: title.to_string(),
                        body,
                        scope,
                        folder_id,
                        provenance: MemoryProvenance {
                            run_id: None,
                            step_id: None,
                            agent_type: None,
                            verified_by_tests: false,
                            source: "external".to_string(),
                        },
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        stale_at: None,
                    },
                    score,
                    via: vec![],
                });
            }
        }

        Ok(hits)
    }

    async fn link(&self, from: i32, to: i32, rel: MemoryRel) -> Result<(), MemoryError> {
        let rel_str = match rel {
            MemoryRel::CausedBy => "caused_by",
            MemoryRel::FixedBy => "fixed_by",
            MemoryRel::RelatesTo => "relates_to",
            MemoryRel::PartOf => "part_of",
            MemoryRel::Supersedes => "supersedes",
        };
        let args = json!({
            "from_id": from,
            "to_id": to,
            "rel": rel_str,
        });

        self.call_tool(&self.config.link_tool, args).await?;
        Ok(())
    }

    async fn get(&self, _id: i32) -> Result<Option<MemoryNode>, MemoryError> {
        Ok(None)
    }

    async fn delete(&self, _id: i32) -> Result<(), MemoryError> {
        Ok(())
    }
}
