use super::*;

const MAX_IPC_REQUEST_BYTES: usize = 1024 * 1024;
static IPC_SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn encode_ipc_response(response: &crate::ipc::IpcResponse) -> String {
    response.to_json_pretty().unwrap_or_else(|error| {
        format!("{{\"type\":\"error\",\"message\":\"failed to encode ipc response: {error}\"}}")
    })
}

impl EvilWm {
    pub(crate) fn init_ipc_listener(
        event_loop: &mut EventLoop<Self>,
    ) -> Result<std::path::PathBuf, Box<dyn Error>> {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
        };

        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let unique = IPC_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket_path =
            runtime_dir.join(format!("evilwm-ipc-{}-{}.sock", std::process::id(), unique));
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let listener = UnixListener::bind(&socket_path).map_err(|error| {
            io::Error::other(format!(
                "failed to create ipc socket {}: {error}",
                socket_path.display()
            ))
        })?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                io::Error::other(format!(
                    "failed to restrict ipc socket permissions {}: {error}",
                    socket_path.display()
                ))
            },
        )?;
        listener.set_nonblocking(true).map_err(|error| {
            io::Error::other(format!(
                "failed to mark ipc socket nonblocking {}: {error}",
                socket_path.display()
            ))
        })?;

        let listener_source = Generic::new(listener, Interest::READ, Mode::Level);
        event_loop
            .handle()
            .insert_source(listener_source, |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((mut stream, _addr)) => {
                            let mut request_json = String::new();
                            if let Err(error) = (&mut stream)
                                .take((MAX_IPC_REQUEST_BYTES + 1) as u64)
                                .read_to_string(&mut request_json)
                            {
                                let _ = stream.write_all(
                                    serde_json::to_string(&crate::ipc::IpcResponse::Error {
                                        message: format!("failed to read ipc request: {error}"),
                                    })
                                    .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"failed to encode error response\"}".into())
                                    .as_bytes(),
                                );
                                continue;
                            }
                            if request_json.len() > MAX_IPC_REQUEST_BYTES {
                                state.trace_ipc_json(
                                    "requests.jsonl",
                                    serde_json::json!({
                                        "raw": request_json,
                                        "status": "too_large",
                                    }),
                                );
                                state.emit_event(
                                    "ipc_request_rejected",
                                    serde_json::json!({
                                        "reason": "too_large",
                                        "bytes": request_json.len(),
                                    }),
                                );
                                let _ = stream.write_all(
                                    serde_json::to_string(&crate::ipc::IpcResponse::Error {
                                        message: format!(
                                            "ipc request exceeds {} bytes",
                                            MAX_IPC_REQUEST_BYTES
                                        ),
                                    })
                                    .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"failed to encode error response\"}".into())
                                    .as_bytes(),
                                );
                                continue;
                            }

                            state.trace_ipc_json(
                                "requests.jsonl",
                                serde_json::json!({
                                    "raw": request_json,
                                    "bytes": request_json.len(),
                                }),
                            );
                            state.emit_event(
                                "ipc_request_received",
                                serde_json::json!({
                                    "bytes": request_json.len(),
                                }),
                            );

                            let response = match crate::ipc::IpcRequest::from_json(&request_json) {
                                Ok(request) => state.handle_ipc_request(request),
                                Err(error) => crate::ipc::IpcResponse::Error {
                                    message: format!("invalid ipc request: {error}"),
                                },
                            };

                            let encoded = encode_ipc_response(&response);
                            state.trace_ipc_json(
                                "responses.jsonl",
                                serde_json::json!({
                                    "raw": encoded,
                                }),
                            );
                            state.emit_event("ipc_response_sent", serde_json::json!({}));
                            let _ = stream.write_all(encoded.as_bytes());
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => {
                            eprintln!("ipc accept failed: {error}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            })
            .map_err(|error| io::Error::other(format!("failed to register ipc socket: {error}")))?;

        Ok(socket_path)
    }

    pub(crate) fn handle_ipc_request(
        &mut self,
        request: crate::ipc::IpcRequest,
    ) -> crate::ipc::IpcResponse {
        match request {
            crate::ipc::IpcRequest::GetRuntimeSnapshot => {
                crate::ipc::IpcResponse::RuntimeSnapshot {
                    snapshot: Box::new(crate::ipc::RuntimeSnapshot::from_live(self)),
                }
            }
            crate::ipc::IpcRequest::Quit => {
                self.loop_signal.stop();
                crate::ipc::IpcResponse::Ok {
                    message: "quitting compositor".into(),
                }
            }
            crate::ipc::IpcRequest::Lock => {
                self.set_session_locked(true);
                crate::ipc::IpcResponse::Ok {
                    message: "session locked".into(),
                }
            }
            crate::ipc::IpcRequest::Unlock => {
                self.set_session_locked(false);
                crate::ipc::IpcResponse::Ok {
                    message: "session unlocked".into(),
                }
            }
            crate::ipc::IpcRequest::Screenshot { path } => {
                if self.is_tty_backend() {
                    crate::ipc::IpcResponse::Error {
                        message:
                            "screenshot capture is currently supported on the nested backend only"
                                .into(),
                    }
                } else {
                    let path = match validate_ipc_screenshot_path(&path) {
                        Ok(path) => path,
                        Err(message) => return crate::ipc::IpcResponse::Error { message },
                    };
                    self.pending_screenshot_path = Some(path.clone());
                    self.emit_event(
                        "screenshot_queued",
                        serde_json::json!({
                            "path": path.display().to_string(),
                            "format": "ppm",
                        }),
                    );
                    self.request_redraw();
                    crate::ipc::IpcResponse::Ok {
                        message: format!("queued screenshot capture to {}", path.display()),
                    }
                }
            }
        }
    }
}

pub(super) fn validate_ipc_screenshot_path(
    path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to resolve screenshot path: {error}"))?
            .join(path)
    };

    let file_name = resolved
        .file_name()
        .ok_or_else(|| "screenshot path must include a file name".to_string())?
        .to_os_string();
    let parent = resolved
        .parent()
        .ok_or_else(|| "screenshot path must have a parent directory".to_string())?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("screenshot parent directory must exist: {error}"))?;
    let canonical = canonical_parent.join(file_name);

    let mut allowed_roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        allowed_roots.push(std::path::PathBuf::from(home));
    }
    allowed_roots.push(std::env::temp_dir());

    if allowed_roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(canonical)
    } else {
        Err(format!(
            "screenshot path must be under $HOME or {}",
            std::env::temp_dir().display()
        ))
    }
}
