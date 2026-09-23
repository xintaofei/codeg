/// Memory backend and integration tests.
#[cfg(test)]
mod sqlite_tests {
    use crate::memory::backend::*;
    use crate::memory::sqlite::LocalSqliteBackend;
    use crate::models::{MemoryRel, MemoryScope};
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_and_get_node() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let backend = LocalSqliteBackend::new(db_path).unwrap();

        let node = NewMemoryNode {
            kind: "decision".to_string(),
            title: "Use async Rust".to_string(),
            body: "Decided to use Tokio for async runtime".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };

        let id = backend.write(node).await.unwrap();
        assert!(id > 0);

        let retrieved = backend.get(id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.kind, "decision");
        assert_eq!(retrieved.title, "Use async Rust");
    }

    #[tokio::test]
    async fn search_by_title_and_body() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let backend = LocalSqliteBackend::new(db_path).unwrap();

        let node = NewMemoryNode {
            kind: "fixed_bug".to_string(),
            title: "Off-by-one in loop".to_string(),
            body: "Fixed loop condition i < n instead of i <= n".to_string(),
            scope: MemoryScope::Project,
            folder_id: Some(42),
            provenance: MemoryProvenance {
                run_id: Some(1),
                step_id: Some("coder".to_string()),
                agent_type: Some("claude".to_string()),
                verified_by_tests: true,
                source: "auto".to_string(),
            },
        };

        backend.write(node).await.unwrap();
        let hits = backend
            .search("loop", MemoryScope::Project, Some(42), 20)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].node.title.contains("loop") || hits[0].node.body.contains("loop"));
    }

    #[tokio::test]
    async fn search_expands_across_edges_up_to_2_hops() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let backend = LocalSqliteBackend::new(db_path).unwrap();

        // Node 1: Contains the keyword "authentication"
        let node1 = NewMemoryNode {
            kind: "decision".to_string(),
            title: "Authentication architecture".to_string(),
            body: "We chose JWT bearer tokens for authentication".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };
        let id1 = backend.write(node1).await.unwrap();

        // Node 2: Does NOT contain "authentication", linked to Node 1 (1 hop)
        let node2 = NewMemoryNode {
            kind: "decision".to_string(),
            title: "Session storage in Redis".to_string(),
            body: "Fast cache layer with TTL expiry".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };
        let id2 = backend.write(node2).await.unwrap();

        // Node 3: Does NOT contain "authentication", linked to Node 2 (2 hops from Node 1)
        let node3 = NewMemoryNode {
            kind: "decision".to_string(),
            title: "Connection pool sizing".to_string(),
            body: "Max 50 active TCP sockets".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };
        let id3 = backend.write(node3).await.unwrap();

        // Node 4: Linked to Node 3 (3 hops away - should not be included)
        let node4 = NewMemoryNode {
            kind: "preference".to_string(),
            title: "Editor formatting".to_string(),
            body: "2 spaces indent".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };
        let id4 = backend.write(node4).await.unwrap();

        // Link 1 -> 2 -> 3 -> 4
        backend.link(id1, id2, MemoryRel::RelatesTo).await.unwrap();
        backend.link(id2, id3, MemoryRel::RelatesTo).await.unwrap();
        backend.link(id3, id4, MemoryRel::RelatesTo).await.unwrap();

        // Search for "authentication"
        let hits = backend
            .search("authentication", MemoryScope::Global, None, 50)
            .await
            .unwrap();

        // Hit 1: Direct match (via = [])
        let hit1 = hits.iter().find(|h| h.node.id == id1);
        assert!(hit1.is_some(), "Direct hit id1 should be found");
        assert_eq!(hit1.unwrap().via, Vec::<i32>::new());

        // Hit 2: 1-hop expansion (via = [id1])
        let hit2 = hits.iter().find(|h| h.node.id == id2);
        assert!(hit2.is_some(), "1-hop linked node id2 should be found");
        assert_eq!(hit2.unwrap().via, vec![id1]);

        // Hit 3: 2-hop expansion (via = [id1, id2])
        let hit3 = hits.iter().find(|h| h.node.id == id3);
        assert!(hit3.is_some(), "2-hop linked node id3 should be found");
        assert_eq!(hit3.unwrap().via, vec![id1, id2]);

        // Hit 4: 3 hops away should NOT be included
        let hit4 = hits.iter().find(|h| h.node.id == id4);
        assert!(hit4.is_none(), "3-hop node id4 should NOT be found");
    }

    #[tokio::test]
    async fn link_nodes() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let backend = LocalSqliteBackend::new(db_path).unwrap();

        let node1 = NewMemoryNode {
            kind: "decision".to_string(),
            title: "First decision".to_string(),
            body: "The first thing we decided".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };

        let node2 = NewMemoryNode {
            kind: "decision".to_string(),
            title: "Second decision".to_string(),
            body: "Based on the first decision".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };

        let id1 = backend.write(node1).await.unwrap();
        let id2 = backend.write(node2).await.unwrap();

        backend.link(id2, id1, MemoryRel::CausedBy).await.unwrap();

        let retrieved = backend.get(id2).await.unwrap().unwrap();
        assert_eq!(retrieved.title, "Second decision");
    }

    #[tokio::test]
    async fn delete_node() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let backend = LocalSqliteBackend::new(db_path).unwrap();

        let node = NewMemoryNode {
            kind: "preference".to_string(),
            title: "Old preference".to_string(),
            body: "No longer relevant".to_string(),
            scope: MemoryScope::Global,
            folder_id: None,
            provenance: MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "user".to_string(),
            },
        };

        let id = backend.write(node).await.unwrap();
        assert!(backend.get(id).await.unwrap().is_some());

        backend.delete(id).await.unwrap();
        assert!(backend.get(id).await.unwrap().is_none());
    }
}

#[cfg(test)]
mod tools_access_tests {
    use std::sync::Arc;
    use tempfile::TempDir;

    use crate::acp::memory_tools::MemoryToolAccess;
    use crate::db::service::memory_kind_service;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::memory::backend::MemoryBackend;
    use crate::memory::sqlite::LocalSqliteBackend;
    use crate::memory::tools::{MemoryBackendAccess, MemoryWriteRequest};
    use crate::models::{
        MemoryBackendKind, MemoryKindDraft, MemoryMode, MemoryScope, MemorySettings,
    };

    #[tokio::test]
    async fn on_request_rejected_without_flag_and_accepted_with_flag() {
        let db = fresh_in_memory_db().await;
        let dir = TempDir::new().unwrap();
        let sqlite = Arc::new(LocalSqliteBackend::new(dir.path().join("mem.db")).unwrap());

        let settings = MemorySettings {
            backend: MemoryBackendKind::LocalSqlite,
            scope: MemoryScope::Global,
            external: None,
        };

        let access =
            MemoryBackendAccess::with_db_and_backend(db.clone(), Some(sqlite), Some(settings));

        // Create an on_request kind
        let kind = memory_kind_service::create(
            &db.conn,
            MemoryKindDraft {
                name: "api_quirk".into(),
                instruction: "Record API quirks".into(),
                mode: MemoryMode::OnRequest,
            },
        )
        .await
        .unwrap();

        // 1. Calling write_entry without user_requested flag (user_requested = false) -> rejected
        let req_without_flag = MemoryWriteRequest {
            parent_connection_id: "conn-1",
            kind: &kind.key,
            title: "Stripe webhook quirk",
            body: "Signature header casing issue",
            links: &[],
            user_requested: false,
            provenance: None,
            folder_id: None,
        };
        let ack1 = access.write_entry(req_without_flag).await;
        assert!(!ack1.ok, "on_request kind without flag must be rejected");
        assert!(ack1.note.unwrap().contains("on_request"));

        // Also test MCP write tool call (which is always user_requested = false)
        let mcp_ack = access
            .write(
                "conn-1",
                &kind.key,
                "Stripe webhook quirk",
                "Details",
                &[],
                false,
            )
            .await;
        assert!(
            !mcp_ack.ok,
            "MCP tool write on on_request kind must be rejected"
        );

        // 2. Calling write_entry with user_requested = true -> accepted
        let req_with_flag = MemoryWriteRequest {
            parent_connection_id: "conn-1",
            kind: &kind.key,
            title: "Stripe webhook quirk",
            body: "Signature header casing issue",
            links: &[],
            user_requested: true,
            provenance: None,
            folder_id: None,
        };
        let ack2 = access.write_entry(req_with_flag).await;
        assert!(
            ack2.ok,
            "on_request kind with user_requested flag must be accepted"
        );
        assert!(ack2.id.is_some());
    }

    #[tokio::test]
    async fn secrets_are_sanitized_before_write() {
        let db = fresh_in_memory_db().await;
        let dir = TempDir::new().unwrap();
        let sqlite = Arc::new(LocalSqliteBackend::new(dir.path().join("mem.db")).unwrap());

        let settings = MemorySettings {
            backend: MemoryBackendKind::LocalSqlite,
            scope: MemoryScope::Global,
            external: None,
        };

        let access = MemoryBackendAccess::with_db_and_backend(
            db.clone(),
            Some(sqlite.clone()),
            Some(settings),
        );

        let req = MemoryWriteRequest {
            parent_connection_id: "conn-1",
            kind: "decision",
            title: "Use OpenAI API",
            body: "Configure with sk-abcdef12345678901234567890 and ghp_abcdef12345678901234567890123456",
            links: &[],
            user_requested: true,
            provenance: None,
            folder_id: None,
        };

        let ack = access.write_entry(req).await;
        assert!(ack.ok);
        let id = ack.id.unwrap();

        let saved = sqlite.get(id).await.unwrap().unwrap();
        assert!(!saved.body.contains("sk-abcdef12345678901234567890"));
        assert!(!saved.body.contains("ghp_abcdef12345678901234567890123456"));
        assert!(saved.body.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn backend_off_rejects_writes() {
        let db = fresh_in_memory_db().await;
        let settings = MemorySettings {
            backend: MemoryBackendKind::Off,
            scope: MemoryScope::Global,
            external: None,
        };

        let access = MemoryBackendAccess::with_db_and_backend(db, None, Some(settings));

        let ack = access
            .write("conn-1", "decision", "Title", "Body", &[], false)
            .await;
        assert!(!ack.ok);
        assert!(ack.note.unwrap().contains("off"));
    }

    #[tokio::test]
    async fn builtin_kind_cannot_be_deleted() {
        let db = fresh_in_memory_db().await;
        let kinds = memory_kind_service::list(&db.conn).await.unwrap();
        let builtin = kinds.iter().find(|k| k.builtin).expect("seeded builtin");

        let res = memory_kind_service::delete(&db.conn, builtin.id).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn custom_kind_appears_in_memory_write_schema() {
        use crate::acp::delegation::companion::{
            update_memory_write_schema, MemoryKindSpec, TOOL_SCHEMA_JSON,
        };
        use serde_json::Value;

        let mut tools: Value = serde_json::from_str(TOOL_SCHEMA_JSON).unwrap();
        let kinds = vec![MemoryKindSpec {
            key: "custom_api_quirk".into(),
            name: "API Quirks".into(),
            instruction: "Record third party quirks".into(),
            mode: "auto".into(),
        }];

        update_memory_write_schema(&mut tools, &kinds);

        let arr = tools.as_array().unwrap();
        let tool = arr.iter().find(|t| t["name"] == "memory_write").unwrap();
        let desc = tool["description"].as_str().unwrap();
        assert!(desc.contains("- custom_api_quirk: Record third party quirks (auto)"));

        let enum_arr = tool["inputSchema"]["properties"]["kind"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(enum_arr, &vec![Value::String("custom_api_quirk".into())]);
    }
}

#[cfg(test)]
mod sanitize_tests {
    use crate::memory::sanitize::sanitize_secrets;

    #[test]
    fn test_sanitize_api_key() {
        let (result, count) = sanitize_secrets("api_key=sk-12345abcde6789012345");
        assert!(result.contains("[REDACTED]"));
        assert!(count > 0);
    }

    #[test]
    fn test_sanitize_standalone_sk_and_ghp() {
        let (r1, c1) =
            sanitize_secrets("OpenAI key is sk-123456789012345678901234567890 in config");
        assert!(r1.contains("[REDACTED]"));
        assert_eq!(c1, 1);

        let (r2, c2) =
            sanitize_secrets("GitHub token is ghp_123456789012345678901234567890123456 for CI");
        assert!(r2.contains("[REDACTED]"));
        assert_eq!(c2, 1);
    }

    #[test]
    fn test_sanitize_password_url() {
        let (result, count) = sanitize_secrets("password=mysecretpass123");
        assert!(result.contains("[REDACTED]"));
        assert_eq!(count, 1);
    }

    #[test]
    fn test_no_false_positives() {
        let text = "We use password-based auth in production";
        let (result, count) = sanitize_secrets(text);
        assert_eq!(count, 0);
        assert_eq!(result, text);
    }
}
