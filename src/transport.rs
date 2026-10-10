//! Product WSS transport. No generic Client platform, URL passthrough or Manager credential logging.
use crate::{
    adapter::Sunshine,
    engine::{ExecutionDeadline, Executor, Journal},
};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::watch,
    time::{Instant, MissedTickBehavior, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{StatusCode, header},
        protocol::WebSocketConfig,
    },
};
use url::Url;
use xcsc::runtime::RetryBackoff;
use xscs_protocol::{
    Binding, Capabilities, ClientMessage, ConfigSnapshot, MAX_MESSAGE_BYTES, ManagerMessage,
    PROTOCOL, Report, WEBSOCKET_SUBPROTOCOL, decode_manager_message,
};
use zeroize::Zeroizing;

fn session_record(
    binding: &Binding,
    event: &str,
    level: xcsc::log::Level,
    code: Option<&str>,
) -> Result<xcsc::log::LogRecord, xcsc::log::LogError> {
    let mut record = xcsc::log::LogRecord::instance(
        "xscc",
        "manager-transport",
        event,
        "Manager session state changed.",
        level,
        &binding.device_id.to_string(),
    )?
    .with_instance_type("sunshine-device")?;
    if let Some(code) = code {
        record = record.with_error_code(code)?;
    }
    Ok(record)
}

fn session_log(
    binding: &Binding,
    event: &str,
    level: xcsc::log::Level,
    code: Option<&str>,
) -> Result<(), TransportError> {
    session_record(binding, event, level, code)
        .and_then(|record| record.emit())
        .map_err(|_| TransportError::Diagnostics)
}

pub use xscs_protocol::CONNECT_PATH;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const PEER_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("invalid Client WSS configuration")]
    Configuration,
    #[error("Client credential rejected or revoked")]
    Revoked,
    #[error("Manager WSS unavailable")]
    Disconnected,
    #[error("invalid Manager protocol message")]
    Protocol,
    #[error("runtime diagnostics unavailable")]
    Diagnostics,
}

#[derive(Clone, Default)]
pub struct HealthObservation {
    pub sunshine_reachable: bool,
    pub configuration: Option<ConfigSnapshot>,
    pub observed_at: Option<Instant>,
}

/// The endpoint must come from protected local provisioning, never from an inbound task.
/// Credentials have no Debug/Serialize implementation and are never put into the URL.
pub struct ManagerConnection {
    endpoint: Url,
    credential: Zeroizing<String>,
}

impl ManagerConnection {
    pub fn new(endpoint: &str, credential: Zeroizing<String>) -> Result<Self, TransportError> {
        let endpoint = Url::parse(endpoint).map_err(|_| TransportError::Configuration)?;
        validate_manager_endpoint(&endpoint)?;
        // Enrollment provisions one independent 256-bit random credential, encoded as lowercase hex.
        if credential.len() != 64
            || !credential
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TransportError::Configuration);
        }
        Ok(Self {
            endpoint,
            credential,
        })
    }

    async fn connect(&self) -> Result<Socket, TransportError> {
        let mut request = self
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| TransportError::Configuration)?;
        let raw = Zeroizing::new(format!("Bearer {}", self.credential.as_str()));
        let mut value = header::HeaderValue::from_str(raw.as_str())
            .map_err(|_| TransportError::Configuration)?;
        value.set_sensitive(true);
        request.headers_mut().insert(header::AUTHORIZATION, value);
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            header::HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
        );
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_MESSAGE_BYTES);
        config.max_frame_size = Some(MAX_MESSAGE_BYTES);
        config.write_buffer_size = 0;
        config.max_write_buffer_size = MAX_MESSAGE_BYTES * 2;
        let (socket, response) = timeout(
            IO_TIMEOUT,
            connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| TransportError::Disconnected)?
        .map_err(classify_handshake)?;
        if response
            .headers()
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            != Some(WEBSOCKET_SUBPROTOCOL)
        {
            return Err(TransportError::Protocol);
        }
        Ok(socket)
    }

    /// Terminal authentication failure stops reconnecting. Other reconnects use xcsc backoff.
    /// The caller owns protected credential/revocation state and retains the Executor across sessions.
    pub async fn run<A: Sunshine + 'static, J: Journal + 'static>(
        &self,
        binding: Binding,
        capabilities: Capabilities,
        executor: Arc<Executor<A, J>>,
        health: watch::Receiver<HealthObservation>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), TransportError> {
        if capabilities.protocol != PROTOCOL {
            return Err(TransportError::Configuration);
        }
        let mut execution = ExecutionState::default();
        for (id, record) in executor
            .pending_results()
            .await
            .map_err(|_| TransportError::Configuration)?
        {
            if let Some(report) = record.report {
                execution.completed.insert(id, (record.fingerprint, report));
            }
        }
        let mut backoff = RetryBackoff::new(Duration::from_secs(1), Duration::from_secs(60), 20)
            .map_err(|_| TransportError::Configuration)?;
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let started = Instant::now();
            let result = tokio::select! {
                biased;
                _ = shutdown.changed() => return Ok(()),
                result = self.session(&binding, &capabilities, executor.clone(), &health, &mut execution) => result,
            };
            crate::runtime_status::observe("manager_session", serde_json::json!("disconnected"));
            let (event, code) = match result {
                Err(TransportError::Revoked) => {
                    ("xscc.session.authorization_failed", "credential_rejected")
                }
                Err(TransportError::Protocol) => {
                    ("xscc.session.protocol_failed", "protocol_invalid")
                }
                Err(TransportError::Configuration) => {
                    ("xscc.session.state_failed", "state_invalid")
                }
                Err(TransportError::Diagnostics) => return result,
                _ => ("xscc.session.disconnected", "connection_failed"),
            };
            session_log(&binding, event, xcsc::log::Level::Warn, Some(code))?;
            if matches!(
                result,
                Err(TransportError::Revoked
                    | TransportError::Configuration
                    | TransportError::Protocol
                    | TransportError::Diagnostics)
            ) {
                return result;
            }
            if started.elapsed() >= Duration::from_secs(60) {
                backoff.reset();
            }
            let delay = backoff
                .next_delay()
                .map_err(|_| TransportError::Configuration)?;
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                _ = tokio::time::sleep(delay) => {},
            }
        }
    }

    async fn session<A: Sunshine + 'static, J: Journal + 'static>(
        &self,
        binding: &Binding,
        capabilities: &Capabilities,
        executor: Arc<Executor<A, J>>,
        health: &watch::Receiver<HealthObservation>,
        execution: &mut ExecutionState,
    ) -> Result<(), TransportError> {
        let mut socket = self.connect().await?;
        send(
            &mut socket,
            ClientMessage::Hello {
                binding: binding.clone(),
                capabilities: capabilities.clone(),
                active_operation: execution.active.as_ref().map(|active| {
                    xscs_protocol::ActiveOperation {
                        operation_id: active.id.clone(),
                        fingerprint: active.fingerprint.clone(),
                    }
                }),
            },
        )
        .await?;
        session_log(
            binding,
            "xscc.session.connected",
            xcsc::log::Level::Info,
            None,
        )?;
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_peer = Instant::now();
        // JoinHandle is retained by run() across sessions; dropping a socket cannot cancel effects.
        for (operation_id, (_, report)) in &execution.completed {
            send(
                &mut socket,
                ClientMessage::Result {
                    operation_id: operation_id.clone(),
                    report: report.clone(),
                },
            )
            .await?;
        }
        let mut last_snapshot = None;
        crate::runtime_status::observe("manager_session", serde_json::json!("connected"));
        loop {
            tokio::select! {
                // Credential revocation/connection close wins over publishing another result.
                biased;
                incoming = socket.next() => {
                    let frame = incoming.ok_or(TransportError::Disconnected)?.map_err(|_| TransportError::Disconnected)?;
                    last_peer = Instant::now();
                    crate::runtime_status::observe("last_peer_at",serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()));
                    match frame {
                        Message::Text(text) => match decode_manager_message(text.as_bytes()).map_err(|_| TransportError::Protocol)? {
                            ManagerMessage::Revoked {} => return Err(TransportError::Revoked),
                            ManagerMessage::ResultAccepted { operation_id, fingerprint, report_digest, finalized } => {
                                if let Some((expected, report)) = execution.completed.get(&operation_id) {
                                    if expected != &fingerprint || xscs_protocol::report_digest(report).ok().as_deref() != Some(&report_digest) { return Err(TransportError::Protocol); }
                                    let executor = executor.clone();
                                    execution.acknowledging.push(tokio::spawn(async move {
                                        let result = executor.acknowledge_final(&operation_id, &fingerprint, &report_digest, finalized).await;
                                        (operation_id, report_digest, result)
                                    }));
                                }
                            }
                            ManagerMessage::Task { mode, task, expires_at_unix_ms, remaining_ms } => {
                                let fingerprint = task.fingerprint().map_err(|_| TransportError::Protocol)?;
                                if let Some(active) = &execution.active {
                                    // A retry can arrive before the first execution finishes. Keep
                                    // the original execution and its eventual result, never run twice.
                                    if active.id == task.operation_id && active.fingerprint == fingerprint { continue; }
                                    return Err(TransportError::Protocol);
                                }
                                if let Some((previous, report)) = execution.completed.get(&task.operation_id) {
                                    if previous != &fingerprint { return Err(TransportError::Protocol); }
                                    if mode != xscs_protocol::DeliveryMode::InspectOnly || !matches!(report, Report::Unknown { .. }) {
                                        send(&mut socket, ClientMessage::Result { operation_id: task.operation_id.clone(), report: report.clone() }).await?;
                                        continue;
                                    }
                                    // An inspect request must collect fresh evidence even when the
                                    // receipt for an earlier uncertain result is still being saved.
                                    execution.completed.remove(&task.operation_id);
                                }
                                let id = task.operation_id.clone();
                                let mutation = !matches!(task.command, xscs_protocol::Command::ReadConfig {} | xscs_protocol::Command::ListApplications {} | xscs_protocol::Command::ListPendingPairings {} | xscs_protocol::Command::ListPairedClients {} | xscs_protocol::Command::ReadLogs { .. } | xscs_protocol::Command::ReadDiagnostics {} | xscs_protocol::Command::ReadVirtualInputStatus {} | xscs_protocol::Command::ReadServiceStatus {});
                                let executor = executor.clone();
                                let deadline = (mode == xscs_protocol::DeliveryMode::Execute).then(|| ExecutionDeadline::received(expires_at_unix_ms, remaining_ms));
                                let handle = tokio::spawn(async move { executor.deliver_before(&task, mode, deadline).await });
                                execution.active = Some(ActiveExecution { id, fingerprint, mutation, handle });
                            }
                        },
                        Message::Ping(bytes) => send_frame(&mut socket, Message::Pong(bytes)).await?,
                        Message::Pong(_) => {},
                        Message::Close(Some(frame)) if matches!(frame.code, tungstenite::protocol::frame::coding::CloseCode::Protocol | tungstenite::protocol::frame::coding::CloseCode::Unsupported | tungstenite::protocol::frame::coding::CloseCode::Policy) => return Err(TransportError::Protocol),
                        Message::Close(_) => return Err(TransportError::Disconnected),
                        _ => return Err(TransportError::Protocol),
                    }
                },
                result = async { (&mut execution.active.as_mut().expect("guarded execution").handle).await }, if execution.active.is_some() => {
                    let active = execution.active.take().expect("completed execution");
                    let report = result.map_err(|_| TransportError::Configuration)?;
                    if active.mutation { execution.mutation_completed = Some(Instant::now()); }
                    execution.completed.insert(active.id.clone(), (active.fingerprint, report.clone()));
                    send(&mut socket, ClientMessage::Result { operation_id: active.id, report }).await?;
                },
                Some(result) = execution.acknowledging.next(), if !execution.acknowledging.is_empty() => {
                    let (id, digest, result) = result.map_err(|_| TransportError::Configuration)?;
                    result.map_err(|_| TransportError::Configuration)?;
                    if execution.completed.get(&id).is_some_and(|(_, report)| xscs_protocol::report_digest(report).ok().as_deref() == Some(&digest)) {
                        execution.completed.remove(&id);
                    }
                },
                _ = tick.tick() => {
                    if last_peer.elapsed() >= PEER_TIMEOUT { return Err(TransportError::Disconnected); }
                    if execution.active.is_none() {
                        // Fetch final receipts after a human resolution, preserving uncertain intent
                        // until the Manager confirms that xcsc has a terminal outcome.
                        for (id, record) in executor.pending_results().await.map_err(|_| TransportError::Configuration)? {
                            if let Some(report) = record.report {
                                execution.completed.entry(id.clone()).or_insert((record.fingerprint, report.clone()));
                                send(&mut socket, ClientMessage::Result { operation_id: id, report }).await?;
                            }
                        }
                    }
                    let observation = health.borrow().clone();
                    let fresh = observation.observed_at.is_some_and(|at| at.elapsed() < Duration::from_secs(30));
                    if let Some(active) = &execution.active {
                        send(&mut socket, ClientMessage::Progress { operation_id: active.id.clone(), fingerprint: active.fingerprint.clone() }).await?;
                    }
                    let configuration = if fresh && observation.observed_at.is_some_and(|at| execution.mutation_completed.is_none_or(|done| at >= done)) && execution.active.as_ref().is_none_or(|active| !active.mutation) {
                        observation.configuration.filter(|snapshot| last_snapshot.as_ref() != Some(&snapshot.revision))
                    } else { None };
                    if let Some(snapshot) = &configuration { last_snapshot = Some(snapshot.revision.clone()); }
                    send(&mut socket, ClientMessage::Heartbeat {
                        sunshine_reachable: fresh.then_some(observation.sunshine_reachable),
                        configuration,
                    }).await?;
                    send_frame(&mut socket, Message::Ping(Vec::new().into())).await?;
                },
            }
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ReceiptCompletion = (String, String, Result<(), crate::engine::JournalError>);
#[derive(Default)]
struct ExecutionState {
    active: Option<ActiveExecution>,
    completed: BTreeMap<String, (String, Report)>,
    mutation_completed: Option<Instant>,
    acknowledging: FuturesUnordered<tokio::task::JoinHandle<ReceiptCompletion>>,
}
struct ActiveExecution {
    id: String,
    fingerprint: String,
    mutation: bool,
    handle: tokio::task::JoinHandle<Report>,
}

async fn send(socket: &mut Socket, message: ClientMessage) -> Result<(), TransportError> {
    let bytes = serde_json::to_string(&message).map_err(|_| TransportError::Protocol)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(TransportError::Protocol);
    }
    send_frame(socket, Message::Text(bytes.into())).await
}

async fn send_frame(socket: &mut Socket, message: Message) -> Result<(), TransportError> {
    timeout(IO_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| TransportError::Disconnected)?
        .map_err(|_| TransportError::Disconnected)
}

pub fn validate_manager_endpoint(url: &Url) -> Result<(), TransportError> {
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != CONNECT_PATH
    {
        return Err(TransportError::Configuration);
    }
    Ok(())
}

fn classify_handshake(error: tungstenite::Error) -> TransportError {
    match error {
        // Only the Manager's own authentication response proves the device
        // credential is invalid. A proxy can generate unrelated 401 or 403.
        tungstenite::Error::Http(response)
            if response.status() == StatusCode::UNAUTHORIZED
                && response
                    .headers()
                    .get("x-xcss-error-code")
                    .is_some_and(|value| value.as_bytes() == b"unauthorized") =>
        {
            TransportError::Revoked
        }
        _ => TransportError::Disconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_distinguishes_manager_authentication_from_proxy_failures() {
        for (status, marker, expected) in [
            (
                StatusCode::UNAUTHORIZED,
                Some("unauthorized"),
                TransportError::Revoked,
            ),
            (StatusCode::UNAUTHORIZED, None, TransportError::Disconnected),
            (
                StatusCode::UNAUTHORIZED,
                Some("forbidden"),
                TransportError::Disconnected,
            ),
            (
                StatusCode::FORBIDDEN,
                Some("unauthorized"),
                TransportError::Disconnected,
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                TransportError::Disconnected,
            ),
        ] {
            let mut builder = tungstenite::http::Response::builder().status(status);
            if let Some(marker) = marker {
                builder = builder.header("x-xcss-error-code", marker);
            }
            let response = builder.body(None).unwrap();
            assert_eq!(
                classify_handshake(tungstenite::Error::Http(Box::new(response))),
                expected
            );
        }
    }

    #[test]
    fn unmarked_manager_401_retries_when_proxy_drops_authorization() {
        let response = tungstenite::http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("content-type", "application/json")
            .body(Some(
                br#"{"code":"unauthorized","message":"unauthorized","retryable":false}"#.to_vec(),
            ))
            .unwrap();
        assert_eq!(
            classify_handshake(tungstenite::Error::Http(Box::new(response))),
            TransportError::Disconnected
        );
    }
}
