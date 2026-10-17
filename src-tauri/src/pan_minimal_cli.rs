//! Console adapter compiled inside the pinned GUI crate. No duplicate token implementation.
use crate::{AppState, AppType, Database, Provider, ProviderService};
use serde_json::{json, Value};
use std::{path::PathBuf, str::FromStr, sync::Arc};

const CONTRACT: &str = "pan-cc-switch-gui/v1";

thread_local! { static GUI_STATE: std::cell::RefCell<Option<AppState>> = const { std::cell::RefCell::new(None) }; }

pub fn main_entry() {
    main_entry_args(std::env::args().skip(1).collect());
}

pub fn main_entry_args(args: Vec<String>) {
    match execute(args) {
        Ok(value) => println!("{}", value),
        Err(message) => {
            // Service errors can contain upstream responses or user credentials.
            eprintln!("{}", json!({"error": message}));
            std::process::exit(1);
        }
    }
}

pub(crate) fn execute_in_gui(args: Vec<String>, state: AppState) -> Result<Value, String> {
    GUI_STATE.with(|slot| *slot.borrow_mut() = Some(state));
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            GUI_STATE.with(|slot| *slot.borrow_mut() = None);
        }
    }
    let _clear = Clear;
    execute(args)
}

fn execute(mut args: Vec<String>) -> Result<Value, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let original_args = args.clone();
    let in_gui = GUI_STATE.with(|slot| slot.borrow().is_some());
    if args == ["--version"] {
        return Ok(
            json!({"contract": CONTRACT, "backend": "GUI", "guiVersion": "4.0.15", "guiRevision": "79e754ab1b6ed50da57dae1d0faecab4b3b52f1c"}),
        );
    }
    let mut app = AppType::Codex;
    if let Some(index) = args.iter().position(|v| v == "--app") {
        let value = args.get(index + 1).ok_or("--app requires an application")?;
        app = AppType::from_str(value).map_err(|_| "Unsupported application")?;
        if !matches!(app, AppType::Codex | AppType::Claude | AppType::Gemini) {
            return Err("Only codex, claude and gemini are supported".into());
        }
        args.drain(index..=index + 1);
    }
    let shape: Vec<&str> = args.iter().map(String::as_str).collect();
    if !matches!(
        shape.as_slice(),
        ["auth", "status", "--json"]
            | ["auth", "list", "--json"]
            | ["auth", "default", _]
            | ["provider", "list", "--json"]
            | ["provider", "switch", _]
            | ["provider", "quota", _, "--json"]
    ) {
        return Err("Unsupported CLI command; no data was opened".into());
    }
    let data_dir = if let Ok(path) = std::env::var("CC_SWITCH_CONFIG_DIR") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("CC_SWITCH_CONFIG_DIR must be absolute".into());
        }
        Some(path)
    } else {
        // The GUI's optional Tauri store override; only select a path, never rewrite the store.
        dirs::config_dir()
            .map(|root| root.join("com.ccswitch.desktop/app_paths.json"))
            .filter(|path| path.exists())
            .map(|path| -> Result<Option<PathBuf>, String> {
                let raw: Value = serde_json::from_slice(
                    &std::fs::read(path).map_err(|_| "Cannot read GUI path settings")?,
                )
                .map_err(|_| "Cannot parse GUI path settings")?;
                match raw.get("app_config_dir_override").and_then(Value::as_str) {
                    Some(path) if !path.is_empty() => {
                        let resolved = if let Some(suffix) =
                            path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
                        {
                            crate::config::get_home_dir().join(suffix)
                        } else {
                            PathBuf::from(path)
                        };
                        if !resolved.is_absolute() || !resolved.is_dir() {
                            return Err("GUI data directory is unavailable".into());
                        }
                        Ok(Some(resolved))
                    }
                    _ => Ok(None),
                }
            })
            .transpose()?
            .flatten()
    };
    if !in_gui {
        crate::app_store::update_cached_override(data_dir);
        if let Some(result) = crate::pan_cli_ipc::forward(&original_args)? {
            return result;
        }
    }
    let _owner = if in_gui {
        None
    } else {
        Some(crate::pan_cli_ipc::claim_owner()?)
    };
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match argv.as_slice() {
        ["auth", "status", "--json"] | ["auth", "list", "--json"] => {
            let manager = manager();
            runtime()?.block_on(async {
                let accounts = manager.list_accounts().await;
                let default = manager.default_account_id().await;
                Ok(json!({"provider": "codex_oauth", "authenticated": !accounts.is_empty(),
                    "default_account_id": default, "accounts": accounts.iter().enumerate().map(|(index,a)| json!({
                        "index": index + 1, "id": a.id, "login": a.login, "reauth_required": a.reauth_required,
                        "is_default": Some(a.id.as_str()) == default.as_deref(), "provider": "codex_oauth",
                        "authenticated_at": a.authenticated_at})).collect::<Vec<_>>()}))
            })
        }
        ["auth", "default", id] => {
            ensure_exclusive_owner()?;
            require_v2_store()?;
            let manager = manager();
            runtime()?.block_on(manager.set_default_account(id)).map_err(|_| "GUI account selection failed")?;
            Ok(json!({"ok": true, "default_account_id": id}))
        }
        ["provider", "list", "--json"] => {
            let state = read_state()?;
            let providers = ProviderService::list(&state, app.clone()).map_err(|_| "GUI provider list failed")?;
            let current = ProviderService::current(&state, app.clone()).map_err(|_| "GUI current provider query failed")?;
            let mut rows: Vec<_> = providers.values().collect();
            rows.sort_by_key(|p| (p.sort_index.unwrap_or(usize::MAX), p.created_at));
            Ok(json!({"app": app.as_str(), "currentProviderId": if current.is_empty() {None} else {Some(&current)},
                "total": rows.len(), "providers": rows.iter().map(|p| {
                    let (base, _) = p.resolve_usage_credentials(&app);
                    json!({"id": p.id, "name": p.name, "apiUrl": clean_origin(&base),
                        "notes": p.notes, "category": p.category,
                        "isCurrent": p.id == current, "sortIndex": p.sort_index, "createdAt": p.created_at})
                }).collect::<Vec<_>>()}))
        }
        ["provider", "switch", id] => {
            ensure_exclusive_owner()?;
            let state = state()?;
            ProviderService::switch(&state, app, id).map_err(|_| "GUI provider switch failed; inspect state before retrying")?;
            Ok(json!({"ok": true}))
        }
        ["provider", "quota", id, "--json"] => {
            ensure_exclusive_owner()?;
            let state = state()?;
            let providers = ProviderService::list(&state, app.clone()).map_err(|_| "GUI provider list failed")?;
            let provider = providers.get(*id).ok_or("Unknown provider ID")?;
            runtime()?.block_on(quota(&state, &app, provider))
        }
        _ => Err("Supported: --version; [--app codex|claude|gemini] provider list --json / provider switch ID / provider quota ID --json; auth status --json / auth default ID".into()),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "Cannot create runtime".into())
}

fn manager() -> Arc<crate::proxy::providers::codex_oauth_auth::CodexOAuthManager> {
    GUI_STATE
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|state| state.codex_oauth_manager.clone())
        })
        .unwrap_or_else(|| {
            Arc::new(
                crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                    crate::config::get_app_config_dir(),
                ),
            )
        })
}

fn state() -> Result<AppState, String> {
    if let Some(state) = GUI_STATE.with(|slot| slot.borrow().clone()) {
        return Ok(state);
    }
    let root = crate::config::get_app_config_dir();
    if !root.join("cc-switch.db").exists() {
        return Err("GUI database is missing; open GUI to initialize it first".into());
    }
    let db = Database::init()
        .map_err(|_| "GUI database initialization failed; no CLI schema fallback is used")?;
    Ok(AppState::new(Arc::new(db)))
}

fn read_state() -> Result<AppState, String> {
    if let Some(state) = GUI_STATE.with(|slot| slot.borrow().clone()) {
        return Ok(state);
    }
    // Listing uses the GUI's DAO and provider service, without GUI startup
    // maintenance (migration, log pruning, pricing seeding or vacuum).
    let path = crate::config::get_app_config_dir().join("cc-switch.db");
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "Cannot open existing GUI database for reading")?;
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| "Cannot read GUI database version")?;
    if !(18..=20).contains(&version) {
        return Err(
            "Open the matching GUI to initialize a supported database before listing".into(),
        );
    }
    Ok(AppState::new(Arc::new(Database {
        conn: std::sync::Mutex::new(connection),
        log_count_cache: std::sync::Mutex::new(None),
    })))
}

fn require_v2_store() -> Result<(), String> {
    let path = crate::config::get_app_config_dir().join("codex_oauth_auth.json");
    let data: Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read GUI account store")?)
            .map_err(|_| "Invalid GUI account store")?;
    if data.get("version").and_then(Value::as_u64) != Some(2) {
        return Err("Account store is not GUI v2; restore a known-good backup or reauthenticate in GUI before changing the default".into());
    }
    Ok(())
}

fn clean_origin(base: &str) -> Option<String> {
    let url = url::Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn ensure_exclusive_owner() -> Result<(), String> {
    if GUI_STATE.with(|slot| slot.borrow().is_some()) {
        return Ok(());
    }
    // GUI's existing test-home contract isolates both credentials and native
    // client files. An unrelated production GUI must not block fixture tests.
    if std::env::var_os("CC_SWITCH_TEST_HOME").is_some_and(|home| {
        let home = PathBuf::from(home);
        home.is_absolute()
            && home != dirs::home_dir().unwrap_or_default()
            && crate::config::get_app_config_dir().starts_with(&home)
    }) {
        return Ok(());
    }
    // GUI holds credential state in memory. It cannot safely share a whole-store
    // writer with a short-lived process. Refuse instead of promising cross-process CAS.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("tasklist.exe")
            .args(["/FI", "IMAGENAME eq cc-switch.exe", "/FO", "CSV", "/NH"])
            .creation_flags(0x08000000)
            .output()
            .map_err(|_| "Cannot verify GUI process ownership")?;
        if !output.status.success() {
            return Err("Cannot verify GUI process ownership".into());
        }
        let my_pid = std::process::id().to_string();
        if String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            let fields: Vec<_> = line.split(',').map(|v| v.trim_matches('"')).collect();
            fields
                .first()
                .is_some_and(|v| v.eq_ignore_ascii_case("cc-switch.exe"))
                && fields.get(1).copied() != Some(my_pid.as_str())
        }) {
            return Err("Close the original CC Switch GUI/legacy CLI before quota refresh or switching; parallel credential writers are not supported".into());
        }
    }
    Ok(())
}

async fn quota(state: &AppState, app: &AppType, provider: &Provider) -> Result<Value, String> {
    let script = provider
        .meta
        .as_ref()
        .and_then(|m| m.usage_script.as_ref())
        .filter(|s| s.enabled);
    let official = script.and_then(|s| s.template_type.as_deref()) == Some("official_subscription");
    let managed = provider.is_codex_oauth();
    let result = if managed {
        require_v2_store()?;
        let manager = &state.codex_oauth_manager;
        let id = match provider
            .meta
            .as_ref()
            .and_then(|m| m.managed_account_id_for("codex_oauth"))
        {
            Some(id) => Some(id),
            None => manager.default_account_id().await,
        }
        .ok_or("No default managed account")?;
        let mut value = crate::commands::query_codex_oauth_quota_for(manager, &id)
            .await
            .map_err(|_| "GUI quota query transport failed")?;
        state.usage_cache.put_codex_oauth(id, value.clone());
        if value.error.is_some() {
            value.error = Some("GUI quota unavailable; check account authentication in GUI".into());
            value.credential_message = value.error.clone();
        }
        json!({"type":"subscription", "quota":value})
    } else if official {
        let current = ProviderService::current(state, app.clone())
            .map_err(|_| "Current provider unavailable")?;
        if current != provider.id {
            return Ok(quota_output(app, provider, "not_available", Value::Null));
        }
        let value = crate::services::subscription::get_subscription_quota(app.as_str())
            .await
            .map_err(|_| "GUI subscription query failed")?;
        json!({"type":"subscription", "quota":value})
    } else if script.is_some() {
        let value = ProviderService::query_usage(state, app.clone(), &provider.id)
            .await
            .map_err(|_| "GUI usage query failed")?;
        json!({"type":"script", "quota":value})
    } else {
        return Ok(quota_output(app, provider, "not_available", Value::Null));
    };
    let success = result
        .pointer("/quota/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(quota_output(
        app,
        provider,
        if success { "ok" } else { "query_failed" },
        result,
    ))
}

fn quota_output(app: &AppType, provider: &Provider, status: &str, result: Value) -> Value {
    json!({"app":app.as_str(), "providerId":provider.id, "providerName":provider.name,
        "status":status, "available":status == "ok", "queriedAt":chrono::Utc::now().timestamp_millis(),
        "result":result, "error": if status == "query_failed" {Some("GUI quota unavailable")} else {None}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_rejects_unknown_command_before_data_access() {
        assert!(execute(vec!["delete-all".into()]).is_err());
    }

    #[test]
    fn version_does_not_initialize_app_or_credentials() {
        assert_eq!(
            execute(vec!["--version".into()]).unwrap()["contract"],
            CONTRACT
        );
    }

    #[test]
    fn origins_never_include_keys_or_userinfo() {
        assert_eq!(
            clean_origin("https://user:secret@example.test/v1?key=secret"),
            Some("https://example.test".into())
        );
    }

    #[test]
    #[serial_test::serial]
    fn gui_default_selection_preserves_every_accounts_identity_and_token_generation() {
        let temporary = tempfile::tempdir().unwrap();
        std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
        crate::app_store::update_cached_override(None);
        let rt = runtime().unwrap();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        rt.block_on(async {
            for account in ["fixture-a", "fixture-b", "fixture-c"] {
                state
                    .codex_oauth_manager
                    .add_test_account_with_user_identity(account, "fake-access", account)
                    .await
                    .unwrap();
            }
        });
        let path = crate::config::get_app_config_dir().join("codex_oauth_auth.json");
        let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(before["version"], 2);
        let selected = execute_in_gui(
            vec!["auth".into(), "default".into(), "fixture-b".into()],
            state.clone(),
        )
        .unwrap();
        assert_eq!(selected["default_account_id"], "fixture-b");
        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["version"], 2);
        assert_eq!(before["accounts"], after["accounts"]);
        assert_eq!(
            rt.block_on(state.codex_oauth_manager.default_account_id())
                .as_deref(),
            Some("fixture-b")
        );
        let listed =
            execute_in_gui(vec!["auth".into(), "status".into(), "--json".into()], state).unwrap();
        assert_eq!(listed["accounts"].as_array().unwrap().len(), 3);
        assert!(!listed.to_string().contains("fake-access"));
        assert!(!listed.to_string().contains("refresh_token"));
        std::env::remove_var("CC_SWITCH_TEST_HOME");
    }

    #[test]
    #[serial_test::serial]
    fn quarantined_v1_store_is_readable_but_never_silently_repaired_by_default_switch() {
        let temporary = tempfile::tempdir().unwrap();
        std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
        crate::app_store::update_cached_override(None);
        let root = crate::config::get_app_config_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("codex_oauth_auth.json");
        let raw = json!({"version":1, "accounts":{"fixture-a":{"account_id":"fixture-a","refresh_token":"fake-old","authenticated_at":1}},"default_account_id":"fixture-a"}).to_string();
        std::fs::write(&path, raw.as_bytes()).unwrap();
        assert!(require_v2_store().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), raw.as_bytes());
        std::env::remove_var("CC_SWITCH_TEST_HOME");
    }
}
