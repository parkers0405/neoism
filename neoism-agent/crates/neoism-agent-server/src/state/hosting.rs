//! Trusted hosting associations. Session IDs/history are never copied or moved.
use super::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostingSession {
    id: String,
    parent_id: Option<String>,
    workspace_id: Option<String>,
    directory: String,
    #[serde(rename = "neoismTenantId", default = "local_tenant")]
    tenant: String,
}
fn local_tenant() -> String {
    "local".into()
}

impl SessionStore {
    #[cfg(test)]
    pub(crate) async fn hosting_history_json_for_test(
        &self,
    ) -> anyhow::Result<Vec<String>> {
        self.db
            .fetch_all("SELECT info_json FROM sessions ORDER BY id", vec![])
            .await?
            .into_iter()
            .map(|row| row.get_str("info_json"))
            .collect()
    }

    pub(crate) async fn associate_host_directory(
        &self,
        directory: &str,
        requested: &str,
    ) -> anyhow::Result<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match self
                .associate_host_directory_once(directory, requested)
                .await
            {
                Err(error)
                    if error
                        .downcast_ref::<turso::Error>()
                        .is_some_and(turso_error_is_busy)
                        && std::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                result => return result,
            }
        }
    }

    async fn associate_host_directory_once(
        &self,
        directory: &str,
        requested: &str,
    ) -> anyhow::Result<String> {
        let root =
            crate::windows_process::canonicalize_path(std::path::Path::new(directory))?;
        anyhow::ensure!(root.is_dir(), "hosting root is not a directory");
        let directory = root.to_string_lossy().into_owned();
        // Serialize the eligibility snapshot and association write against every
        // other store writer. Do not call execute_transaction while holding this gate.
        let _writer = self.db.lock_writer().await;
        let mut conn = self.db.database.connect()?;
        let tx = conn
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await?;
        tx.execute("INSERT INTO hosted_chat_directories(directory, workspace_id) VALUES (?, ?) ON CONFLICT(directory) DO NOTHING", vec![text(&directory), text(requested)]).await?;
        let mut rows = tx
            .query(
                "SELECT workspace_id FROM hosted_chat_directories WHERE directory = ?",
                vec![text(&directory)],
            )
            .await?;
        let workspace: String = rows
            .next()
            .await?
            .context("missing hosting namespace")?
            .get(0)?;
        drop(rows);
        let mut rows = tx
            .query("SELECT info_json FROM sessions", Vec::<SqlValue>::new())
            .await?;
        let mut sessions = Vec::<HostingSession>::new();
        while let Some(row) = rows.next().await? {
            sessions.push(serde_json::from_str(&row.get::<String>(0)?)?);
        }
        drop(rows);
        let mut rows = tx.query(
            "SELECT session_id, workspace_id FROM hosted_chat_sessions UNION ALL SELECT session_id, workspace_id FROM hosted_chat_revocations",
            Vec::<SqlValue>::new(),
        ).await?;
        let mut owners: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        while let Some(row) = rows.next().await? {
            owners.entry(row.get(0)?).or_default().push(row.get(1)?);
        }
        drop(rows);
        let by_id: std::collections::HashMap<_, _> =
            sessions.iter().map(|s| (s.id.as_str(), s)).collect();
        let mut children: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for session in &sessions {
            if let Some(parent) = session.parent_id.as_deref() {
                children
                    .entry(parent)
                    .or_default()
                    .push(session.id.as_str());
            }
        }
        let mut families = Vec::new();
        for session in &sessions {
            if session.parent_id.is_some()
                || !((session.workspace_id.is_none() && session.tenant == "local")
                    || (session.workspace_id.as_deref() == Some(workspace.as_str())
                        && session.tenant == format!("workspace:{workspace}")))
            {
                continue;
            }
            let previously_hosted = session.workspace_id.as_deref()
                == Some(workspace.as_str())
                || owners
                    .get(&session.id)
                    .is_some_and(|owners| owners.iter().any(|owner| owner == &workspace));
            if !previously_hosted
                && crate::windows_process::canonicalize_path(std::path::Path::new(
                    &session.directory,
                ))
                .ok()
                .as_ref()
                    != Some(&root)
            {
                continue;
            }
            let mut ids = std::collections::HashSet::new();
            let mut pending = vec![session.id.as_str()];
            while let Some(id) = pending.pop() {
                if ids.insert(id.to_string()) {
                    pending.extend(children.get(id).into_iter().flatten().copied());
                }
            }
            // Never steal owned descendants, including in families that will
            // otherwise be ineligible for this directory capability.
            anyhow::ensure!(ids.iter().filter_map(|id| by_id.get(id.as_str())).all(|s| {
                s.workspace_id.is_none() && s.tenant == "local"
                    || (s.workspace_id.as_deref() == Some(workspace.as_str()) && s.tenant == format!("workspace:{workspace}"))
            }), "local conversation family contains a foreign-owned child; hosting association not changed");
            anyhow::ensure!(
                ids.iter().all(|id| owners
                    .get(id)
                    .is_none_or(|owners| owners.iter().all(|owner| owner == &workspace))),
                "conversation already associated with a different hosted directory"
            );
            // Historical local tasks may have run outside the selected root.
            // Leave that ENTIRE family private rather than splitting it, widening
            // the capability, or blocking unrelated eligible conversations.
            if !ids.iter().filter_map(|id| by_id.get(id.as_str())).all(|s| {
                crate::windows_process::canonicalize_path(std::path::Path::new(
                    &s.directory,
                ))
                .is_ok_and(|p| p.starts_with(&root))
            }) {
                // Revocation is authoritative even if a previous hydrated overlay
                // was persisted in info_json. Preserve history and local access.
                for id in ids {
                    tx.execute("DELETE FROM hosted_chat_sessions WHERE session_id = ? AND workspace_id = ?", vec![text(&id), text(&workspace)]).await?;
                    tx.execute("INSERT INTO hosted_chat_revocations(session_id, workspace_id) VALUES (?, ?) ON CONFLICT(session_id) DO NOTHING", vec![text(&id), text(&workspace)]).await?;
                }
                continue;
            }
            families.extend(ids);
        }
        for id in families {
            tx.execute("DELETE FROM hosted_chat_revocations WHERE session_id = ? AND workspace_id = ?", vec![text(&id), text(&workspace)]).await?;
            tx.execute("INSERT INTO hosted_chat_sessions(session_id, workspace_id) VALUES (?, ?) ON CONFLICT(session_id) DO NOTHING", vec![text(&id), text(&workspace)]).await?;
            let mut rows = tx
                .query(
                    "SELECT workspace_id FROM hosted_chat_sessions WHERE session_id = ?",
                    vec![text(id)],
                )
                .await?;
            let owner: String = rows
                .next()
                .await?
                .context("missing session association")?
                .get(0)?;
            anyhow::ensure!(
                owner == workspace,
                "conversation already associated with a different hosted directory"
            );
        }
        tx.commit().await?;
        Ok(workspace)
    }

    pub(crate) async fn revoked_artifact_namespace(
        &self,
        session_id: &str,
        tenant: &str,
    ) -> anyhow::Result<bool> {
        Ok(self.db.fetch_optional(
            "SELECT session_id FROM hosted_chat_revocations WHERE session_id = ? AND 'workspace:' || workspace_id = ?",
            vec![text(session_id), text(tenant)],
        ).await?.is_some())
    }

    pub(crate) async fn hydrate_host_associations(
        &self,
        sessions: &mut [SessionInfo],
    ) -> anyhow::Result<()> {
        self.db.hydrate_host_associations(sessions).await
    }
}

impl Db {
    /// Overlay authoritative associations on reads, not on history. Remove the
    /// reserved local-access marker first: imports/extra JSON cannot grant access.
    pub(crate) async fn hydrate_host_associations(
        &self,
        sessions: &mut [SessionInfo],
    ) -> anyhow::Result<()> {
        if sessions.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::new();
        let mut revoked_rows = Vec::new();
        // Single-session timeline reads must not scan all hosted history.
        for chunk in sessions.chunks(200) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            rows.extend(self.fetch_all(
                &format!("SELECT session_id, workspace_id FROM hosted_chat_sessions WHERE session_id IN ({placeholders})"),
                chunk.iter().map(|session| text(session.id.to_string())).collect(),
            ).await?);
            revoked_rows.extend(self.fetch_all(
                &format!("SELECT session_id, workspace_id FROM hosted_chat_revocations WHERE session_id IN ({placeholders})"),
                chunk.iter().map(|session| text(session.id.to_string())).collect(),
            ).await?);
        }
        let mut revocations = std::collections::HashMap::new();
        for row in revoked_rows {
            revocations.insert(row.get_str("session_id")?, row.get_str("workspace_id")?);
        }
        let mut associations = std::collections::HashMap::new();
        for row in rows {
            associations.insert(row.get_str("session_id")?, row.get_str("workspace_id")?);
        }
        let rows = self
            .fetch_all(
                "SELECT directory, workspace_id FROM hosted_chat_directories",
                vec![],
            )
            .await?;
        let mut namespaces = std::collections::HashMap::new();
        for row in rows {
            namespaces.insert(row.get_str("workspace_id")?, row.get_str("directory")?);
        }
        for session in sessions {
            session.extra.remove(crate::caller::HOST_LOCAL_ACCESS_KEY);
            if let Some(workspace) = revocations.get(session.id.as_str()) {
                if session.workspace_id.is_none()
                    && crate::caller::session_tenant(session) == "local"
                {
                    continue;
                }
                if session.workspace_id.as_deref() == Some(workspace.as_str())
                    && crate::caller::session_tenant(session)
                        == format!("workspace:{workspace}")
                {
                    session.workspace_id = None;
                    session.extra.remove(crate::caller::TENANT_EXTRA_KEY);
                    continue;
                }
            }
            if let Some(workspace) = associations.get(session.id.as_str()) {
                // A persisted overlay or the original unowned local row is OK;
                // an explicit reassignment elsewhere is never overridden.
                if (session.workspace_id.is_none()
                    && crate::caller::session_tenant(session) == "local")
                    || (session.workspace_id.as_deref() == Some(workspace.as_str())
                        && crate::caller::session_tenant(session)
                            == format!("workspace:{workspace}"))
                {
                    session.workspace_id = Some(workspace.clone());
                    if let Ok(directory) = crate::windows_process::canonicalize_path(
                        std::path::Path::new(&session.directory),
                    ) {
                        session.directory = directory.to_string_lossy().into_owned();
                    }
                    session.extra.insert(
                        crate::caller::TENANT_EXTRA_KEY.into(),
                        serde_json::json!(format!("workspace:{workspace}")),
                    );
                }
            }
            if let Some(workspace) = session.workspace_id.as_ref() {
                if crate::caller::session_tenant(session)
                    == format!("workspace:{workspace}")
                    && namespaces.get(workspace).is_some_and(|root| {
                        crate::windows_process::canonicalize_path(std::path::Path::new(
                            &session.directory,
                        ))
                        .is_ok_and(|p| p.starts_with(root))
                    })
                {
                    session.extra.insert(
                        crate::caller::HOST_LOCAL_ACCESS_KEY.into(),
                        serde_json::json!(true),
                    );
                }
            }
        }
        Ok(())
    }
}
