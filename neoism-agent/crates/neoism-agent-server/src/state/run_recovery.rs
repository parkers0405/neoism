use super::*;

impl AppState {
    pub(crate) async fn claim_prompt_run(
        &self,
        session_id: &str,
        run_id: &str,
        request: &PromptRequest,
    ) -> anyhow::Result<()> {
        let _commit = self.event_commit_guard().await?;
        self.inner.store.claim_prompt_run(session_id, run_id, request).await
    }
}

impl SessionStore {
    /// Write-ahead intent for the existing prompt loop. The original request
    /// and retry budget survive replacement runs until terminal settlement.
    pub(crate) async fn claim_prompt_run(
        &self,
        session_id: &str,
        run_id: &str,
        request: &PromptRequest,
    ) -> anyhow::Result<()> {
        let now = store_i64(crate::now_millis());
        self.db.execute_transaction(vec![
            ("INSERT INTO session_runs (id, session_id, status, created, updated) VALUES (?, ?, 'running', ?, ?) ON CONFLICT(id) DO NOTHING".into(), vec![text(run_id), text(session_id), int(now), int(now)]),
            ("INSERT INTO session_run_claims (session_id, run_id, request_json) VALUES (?, ?, ?) ON CONFLICT(session_id) DO UPDATE SET run_id = excluded.run_id, attempts = CASE WHEN json_extract(session_run_claims.request_json, '$.messageId') = json_extract(excluded.request_json, '$.messageId') THEN session_run_claims.attempts ELSE 0 END, request_json = excluded.request_json".into(), vec![text(session_id), text(run_id), text(serde_json::to_string(request)?)]),
        ]).await
    }

    pub(crate) async fn suspend_prompt_runs(&self) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE session_runs SET status = 'suspended' WHERE id IN (SELECT run_id FROM session_run_claims)",
            vec![],
        ).await?;
        Ok(())
    }

    pub(crate) async fn claimed_run_ids(&self) -> anyhow::Result<Vec<String>> {
        self.db.fetch_all("SELECT run_id FROM session_run_claims", vec![]).await?
            .into_iter().map(|row| row.get_str("run_id")).collect()
    }

    pub(crate) async fn cancel_prompt_claim(&self, session_id: &str) -> anyhow::Result<()> {
        self.db.execute_transaction(vec![
            ("DELETE FROM prompt_queue WHERE session_id = ? AND (id LIKE 'restart:%' OR json_extract(request_json, '$.messageId') = (SELECT json_extract(request_json, '$.messageId') FROM session_run_claims WHERE session_id = ?))".into(), vec![text(session_id), text(session_id)]),
            ("DELETE FROM session_run_claims WHERE session_id = ?".into(), vec![text(session_id)]),
        ]).await
    }

    /// Called before workers start. Hosting must guarantee the previous owner
    /// is dead. Recovery is at-least-once history continuation, not tool replay
    /// or exactly-once external effects. The queue is only a wakeup token: the
    /// claim remains durable even after that token is handed to a new run.
    pub(crate) async fn prepare_run_recovery(&self) -> anyhow::Result<()> {
        // Process-local waiters died with the old host. Do not expose buttons
        // that can no longer deliver an answer; the resumed loop can ask anew.
        self.db.execute(
            "UPDATE interaction_requests SET state = 'cancelled', response_json = ?, updated = ? WHERE state = 'pending' AND session_id IN (SELECT session_id FROM session_run_claims)",
            vec![text(json!({"reason": "server_restarted"}).to_string()), int(store_i64(crate::now_millis()))],
        ).await?;
        for row in self.db.fetch_all(
            "SELECT session_id, run_id, request_json, attempts FROM session_run_claims", vec![],
        ).await? {
            let session_id = row.get_str("session_id")?;
            let run_id = row.get_str("run_id")?;
            let attempts = row.get_i64("attempts")?;
            if attempts >= 10 {
                self.db.execute_transaction(vec![
                    ("UPDATE session_runs SET status = 'error', error_json = ? WHERE id = ?".into(), vec![text(json!({"message": "Restart recovery exhausted after ten attempts"}).to_string()), text(&run_id)]),
                    ("DELETE FROM prompt_queue WHERE session_id = ? AND (id LIKE 'restart:%' OR json_extract(request_json, '$.messageId') = (SELECT json_extract(request_json, '$.messageId') FROM session_run_claims WHERE session_id = ?))".into(), vec![text(&session_id), text(&session_id)]),
                    ("DELETE FROM session_run_claims WHERE session_id = ? AND run_id = ?".into(), vec![text(&session_id), text(&run_id)]),
                ]).await?;
                tracing::warn!(%session_id, "restart recovery attempt limit reached");
                continue;
            }
            let request_json = row.get_str("request_json")?;
            let request: PromptRequest = serde_json::from_str(&request_json)?;
            let message_id = request.message_id.as_ref()
                .ok_or_else(|| anyhow::anyhow!("recoverable run has no message identity"))?;
            let id = format!("restart:{}", Id::ascending(IdKind::Event));
            self.db.execute_transaction(vec![
                ("UPDATE session_run_claims SET attempts = attempts + 1 WHERE session_id = ? AND run_id = ?".into(), vec![text(&session_id), text(&run_id)]),
                // Reuse a saved pre-handoff admission if it still exists. A
                // continuation otherwise precedes later queued user turns.
                ("INSERT INTO prompt_queue (id, session_id, position, request_json, created, kind, delivery, saved_message_id) SELECT ?, ?, COALESCE((SELECT MIN(position) - 1 FROM prompt_queue WHERE session_id = ?), 0), ?, ?, 'prompt', 'continue', ? WHERE NOT EXISTS (SELECT 1 FROM prompt_queue WHERE session_id = ? AND kind = 'prompt' AND json_extract(request_json, '$.messageId') = ?)".into(), vec![text(id), text(&session_id), text(&session_id), text(request_json), int(store_i64(crate::now_millis())), text(message_id.as_str()), text(&session_id), text(message_id.as_str())]),
            ]).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (SessionStore, String, PromptRequest) {
        let path = std::env::temp_dir().join(format!("restart-{}.db", Id::ascending(IdKind::Event)));
        let store = SessionStore::open(path).await.unwrap();
        let session_id = Id::ascending(IdKind::Session).to_string();
        let session: SessionInfo = serde_json::from_value(json!({
            "id": session_id, "slug": "restart", "projectId": "global",
            "directory": "/tmp", "title": "restart", "version": "test",
            "time": {"created": 1, "updated": 1}
        })).unwrap();
        store.insert_session(&session).await.unwrap();
        let request: PromptRequest = serde_json::from_value(json!({
            "messageId": Id::ascending(IdKind::Message),
            "parts": [{"type": "text", "text": "continue task"}]
        })).unwrap();
        (store, session_id, request)
    }

    #[tokio::test]
    async fn restart_claim_survives_handoff_and_shutdown_until_terminal_commit() {
        let (store, session, request) = setup().await;
        store.claim_prompt_run(&session, "old", &request).await.unwrap();
        store.suspend_prompt_runs().await.unwrap();
        store.finish_run("old", "interrupted", None).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        let (id, saved, _, already_saved) = store.peek_prompt_admission(&session, false).await.unwrap().unwrap();
        assert!(already_saved);
        assert_eq!(saved.message_id, request.message_id);
        store.claim_prompt_run(&session, "resumed", &saved).await.unwrap();
        store.handoff_prompt_admission(&id, &session, "resumed").await.unwrap();
        // An old run's delayed cleanup cannot release its successor's claim.
        store.finish_run("old", "completed", None).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        assert_eq!(store.queued_prompt_count(&session).await.unwrap(), 1);
        let (id, _, _, _) = store.peek_prompt_admission(&session, false).await.unwrap().unwrap();
        store.claim_prompt_run(&session, "last", &request).await.unwrap();
        store.handoff_prompt_admission(&id, &session, "last").await.unwrap();
        store.finish_run("last", "completed", None).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        assert_eq!(store.queued_prompt_count(&session).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn restart_budget_is_durable_and_stop_prevents_recovery() {
        let (store, session, request) = setup().await;
        store.claim_prompt_run(&session, "first", &request).await.unwrap();
        for attempt in 0..10 {
            store.prepare_run_recovery().await.unwrap();
            let (id, saved, _, _) = store.peek_prompt_admission(&session, false).await.unwrap().unwrap();
            let run = format!("attempt-{attempt}");
            store.claim_prompt_run(&session, &run, &saved).await.unwrap();
            store.handoff_prompt_admission(&id, &session, &run).await.unwrap();
        }
        store.prepare_run_recovery().await.unwrap();
        assert_eq!(store.queued_prompt_count(&session).await.unwrap(), 0);
        assert!(store.session_run_statuses(&session).await.unwrap().iter().any(|(id, status)| id == "attempt-9" && status == "error"));

        store.claim_prompt_run(&session, "cancelled", &request).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        store.cancel_prompt_claim(&session).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        assert_eq!(store.queued_prompt_count(&session).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn restart_before_handoff_reuses_admission_and_failed_claim_rolls_back() {
        let (store, session, request) = setup().await;
        store.db.execute("INSERT INTO prompt_queue (id, session_id, position, request_json, created, kind, delivery, saved_message_id) VALUES ('original', ?, 0, ?, 1, 'prompt', 'queue', ?)", vec![text(&session), text(serde_json::to_string(&request).unwrap()), text(request.message_id.as_ref().unwrap().as_str())]).await.unwrap();
        store.claim_prompt_run(&session, "first", &request).await.unwrap();
        store.prepare_run_recovery().await.unwrap();
        assert_eq!(store.queued_prompt_count(&session).await.unwrap(), 1);
        assert_eq!(store.peek_prompt_admission(&session, false).await.unwrap().unwrap().0, "original");

        store.db.execute("ALTER TABLE session_run_claims RENAME TO unavailable_claims", vec![]).await.unwrap();
        assert!(store.claim_prompt_run(&session, "failed", &request).await.is_err());
        assert!(!store.session_run_statuses(&session).await.unwrap().iter().any(|(id, _)| id == "failed"));
    }
}