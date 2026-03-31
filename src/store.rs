//! Persistence layer — reads and writes JSON files in the `.browsectl/` directory.
//!
//! Two data stores are managed:
//!
//! - **Session store** (`.browsectl/sessions.json`): tracks live browser sessions
//!   so they survive CLI invocations.  Functions: [`read_store`], [`write_store`],
//!   [`upsert`], [`remove`], [`set_default`].
//! - **Setup info** (`.browsectl/setup.json`): caches platform, browser, and driver
//!   detection results from the `setup` command so that session creation can
//!   skip re-detection.  Functions: [`read_setup_info`], [`write_setup_info`].

use crate::types::{
    SessionStoreData, SetupInfo, StoredSession, now_iso, session_store_path, setup_info_path,
};
use anyhow::{Result, bail};

pub async fn read_store() -> Result<SessionStoreData> {
    let path = session_store_path();
    if !path.exists() {
        return Ok(SessionStoreData::default());
    }
    let raw = tokio::fs::read_to_string(&path).await?;
    match serde_json::from_str::<SessionStoreData>(&raw) {
        Ok(store) => Ok(store),
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {e} — starting with empty store",
                path.display()
            );
            Ok(SessionStoreData::default())
        }
    }
}

pub async fn write_store(store: &SessionStoreData) -> Result<()> {
    let path = session_store_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(store)?;
    tokio::fs::write(path, format!("{raw}\n")).await?;
    Ok(())
}

pub async fn upsert(session_id: &str, payload: StoredSession, set_default: bool) -> Result<()> {
    let mut store = read_store().await?;
    let mut session = payload;
    session.updated_at = now_iso();
    store.sessions.insert(session_id.to_string(), session);
    if set_default || store.default_session_id.is_none() {
        store.default_session_id = Some(session_id.to_string());
    }
    write_store(&store).await
}

pub async fn remove(session_id: &str) -> Result<SessionStoreData> {
    let mut store = read_store().await?;
    store.sessions.remove(session_id);
    if store.default_session_id.as_deref() == Some(session_id) {
        store.default_session_id = store.sessions.keys().next().cloned();
    }
    write_store(&store).await?;
    Ok(store)
}

pub async fn set_default(session_id: &str) -> Result<()> {
    let mut store = read_store().await?;
    if !store.sessions.contains_key(session_id) {
        bail!("Session not found: {session_id}");
    }
    store.default_session_id = Some(session_id.to_string());
    write_store(&store).await
}

// ---------------------------------------------------------------------------
// Setup info (.browsectl/setup.json)
// ---------------------------------------------------------------------------

pub async fn read_setup_info() -> Result<SetupInfo> {
    let path = setup_info_path();
    if !path.exists() {
        return Ok(SetupInfo::default());
    }
    let raw = tokio::fs::read_to_string(&path).await?;
    match serde_json::from_str::<SetupInfo>(&raw) {
        Ok(info) => Ok(info),
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {e} — returning empty setup info",
                path.display()
            );
            Ok(SetupInfo::default())
        }
    }
}

pub async fn write_setup_info(info: &SetupInfo) -> Result<()> {
    let path = setup_info_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(info)?;
    tokio::fs::write(path, format!("{raw}\n")).await?;
    Ok(())
}
