//! Per-user local CLI entry to the already-running GUI business state.
//! Stdout and pipe payloads never include provider settings or OAuth tokens.
use serde_json::{json, Value};
use std::time::Duration;

/// Lifetime owner shared by the new GUI and standalone CLI. The original GUI
/// does not use this lock and must be closed before standalone mutations.
pub(crate) struct CliOwner {
    _file: std::fs::File,
}

pub(crate) fn claim_owner() -> Result<CliOwner, String> {
    use fs2::FileExt;
    let root = crate::config::get_app_config_dir();
    std::fs::create_dir_all(&root).map_err(|_| "Cannot open GUI data directory")?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(root.join(".gui-cli-owner.lock"))
        .map_err(|_| "Cannot open GUI/CLI ownership lock")?;
    file.try_lock_exclusive()
        .map_err(|_| "GUI/CLI is starting or busy; retry a read later, no fallback was executed")?;
    Ok(CliOwner { _file: file })
}

#[cfg(windows)]
fn address() -> String {
    use sha2::{Digest, Sha256};
    let root = crate::config::get_app_config_dir()
        .to_string_lossy()
        .to_lowercase();
    let hash = format!("{:x}", Sha256::digest(root.as_bytes()));
    format!(r"\\.\pipe\cc-switch-gui-cli-{}", &hash[..24])
}

pub(crate) fn forward(args: &[String]) -> Result<Option<Result<Value, String>>, String> {
    #[cfg(windows)]
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "CLI IPC runtime failed")?;
        rt.block_on(async {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            use tokio::net::windows::named_pipe::ClientOptions;
            let path = address();
            let until = std::time::Instant::now() + Duration::from_secs(3);
            let pipe = loop {
                match ClientOptions::new().open(&path) {
                    Ok(pipe) => break pipe,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error)
                        if error.raw_os_error() == Some(231)
                            && std::time::Instant::now() < until =>
                    {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                    }
                    Err(_) => {
                        return Err(
                            "GUI pipe unavailable; no standalone fallback was attempted".into()
                        )
                    }
                }
            };
            let exchange = async {
                let (reader, mut writer) = tokio::io::split(pipe);
                let payload =
                    serde_json::to_vec(&json!({"protocol":"cc-switch-cli/v1", "arguments":args}))
                        .map_err(|_| "Invalid CLI request")?;
                if payload.len() > 32768 {
                    return Err("CLI request is too large".into());
                }
                writer
                    .write_all(&payload)
                    .await
                    .map_err(|_| "GUI receipt unknown; do not retry a switch")?;
                writer
                    .write_all(b"\n")
                    .await
                    .map_err(|_| "GUI receipt unknown; do not retry a switch")?;
                let mut line = String::new();
                BufReader::new(reader.take(2 * 1024 * 1024))
                    .read_line(&mut line)
                    .await
                    .map_err(|_| "GUI outcome unknown; do not replay")?;
                let response: Value = serde_json::from_str(&line)
                    .map_err(|_| "Invalid GUI response; outcome unknown")?;
                if let Some(error) = response.get("error").and_then(Value::as_str) {
                    return Err(error.to_owned());
                }
                response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Missing GUI result; outcome unknown".into())
            };
            let result = tokio::time::timeout(Duration::from_secs(28), exchange)
                .await
                .map_err(|_| "GUI request timed out; outcome unknown, do not replay")?;
            Ok(Some(result))
        })
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        Ok(None)
    }
}

pub(crate) fn start(app: tauri::AppHandle) {
    #[cfg(windows)]
    {
        use tauri::{Emitter, Manager};
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        // Reserve before spawning. No TCP/HTTP listener and no second credential manager.
        let path = address();
        tauri::async_runtime::spawn(async move {
            let mut pipe = match ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&path)
            {
                Ok(pipe) => pipe,
                Err(error) => {
                    log::error!("CLI pipe could not start: {error}");
                    return;
                }
            };
            loop {
                if pipe.connect().await.is_err() {
                    break;
                }
                // Sequential admission: one in-flight CLI request in this GUI process.
                let (reader, mut writer) = tokio::io::split(pipe);
                let mut line = String::new();
                let read = tokio::time::timeout(
                    Duration::from_secs(3),
                    BufReader::new(reader.take(32769)).read_line(&mut line),
                )
                .await;
                let args = if matches!(read, Ok(Ok(_))) && line.len() <= 32768 {
                    serde_json::from_str::<Value>(&line)
                        .ok()
                        .filter(|v| v["protocol"] == "cc-switch-cli/v1")
                        .and_then(|v| {
                            serde_json::from_value::<Vec<String>>(v["arguments"].clone()).ok()
                        })
                } else {
                    None
                };
                let response = if let Some(args) = args {
                    let state = app.state::<crate::AppState>().inner().clone();
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        crate::pan_minimal_cli::execute_in_gui(args, state)
                    })
                    .await;
                    match result {
                        Ok(Ok(value)) => {
                            crate::tray::schedule_tray_refresh(&app);
                            let _ = app.emit("cli-state-updated", json!({"source":"cli"}));
                            json!({"result":value})
                        }
                        Ok(Err(error)) => json!({"error":error}),
                        Err(_) => {
                            json!({"error":"GUI command ended unexpectedly; outcome unknown"})
                        }
                    }
                } else {
                    json!({"error":"Invalid CLI IPC request"})
                };
                let mut output = serde_json::to_vec(&response).unwrap_or_default();
                output.push(b'\n');
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), writer.write_all(&output)).await;
                // Dropping the connected pipe also closes clients with uncertain results.
                drop(writer);
                pipe = match ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&path)
                {
                    Ok(next) => next,
                    Err(_) => break,
                };
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn named_pipe_selects_default_in_the_same_gui_manager_without_dropping_fields() {
        use std::sync::Arc;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let temporary = tempfile::tempdir().unwrap();
        std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
        crate::app_store::update_cached_override(None);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = crate::AppState::new(Arc::new(crate::Database::memory().unwrap()));
        runtime.block_on(async {
            for id in ["fixture-a", "fixture-b"] {
                state
                    .codex_oauth_manager
                    .add_test_account_with_user_identity(id, "fake-access", id)
                    .await
                    .unwrap();
            }
        });
        let path = crate::config::get_app_config_dir().join("codex_oauth_auth.json");
        let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let shared = state.clone();
        let name = address();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let pipe = ServerOptions::new()
                    .first_pipe_instance(true)
                    .reject_remote_clients(true)
                    .create(&name)
                    .unwrap();
                ready_tx.send(()).unwrap();
                pipe.connect().await.unwrap();
                let (reader, mut writer) = tokio::io::split(pipe);
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let message: Value = serde_json::from_str(&line).unwrap();
                let args: Vec<String> =
                    serde_json::from_value(message["arguments"].clone()).unwrap();
                let result = tokio::task::spawn_blocking(move || {
                    crate::pan_minimal_cli::execute_in_gui(args, shared)
                })
                .await
                .unwrap()
                .unwrap();
                let payload = json!({"result":result}).to_string() + "\n";
                writer.write_all(payload.as_bytes()).await.unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = forward(&["auth".into(), "default".into(), "fixture-b".into()])
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result["default_account_id"], "fixture-b");
        server.join().unwrap();
        assert_eq!(
            runtime
                .block_on(state.codex_oauth_manager.default_account_id())
                .as_deref(),
            Some("fixture-b")
        );
        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(before["accounts"], after["accounts"]);
        assert_eq!(after["version"], 2);
        std::env::remove_var("CC_SWITCH_TEST_HOME");
    }

    #[test]
    #[serial_test::serial]
    fn a_second_headless_owner_cannot_write_the_gui_store() {
        let temporary = tempfile::tempdir().unwrap();
        std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
        crate::app_store::update_cached_override(None);
        let owner = claim_owner().unwrap();
        assert!(claim_owner().is_err());
        drop(owner);
        assert!(claim_owner().is_ok());
        std::env::remove_var("CC_SWITCH_TEST_HOME");
    }
}
