//! Product WSS transport. No generic Client platform, URL passthrough or Manager credential logging.
use crate::{
    adapter::Sunshine,
    engine::{Executor, Journal},
};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use sarmg_client_runtime::RetryBackoff;
use sarmg_client_secure_http::Url;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use sunshine_client_protocol::{
    Binding, Capabilities, ClientMessage, ConfigSnapshot, MAX_MESSAGE_BYTES, ManagerMessage,
    PROTOCOL, Report, WEBSOCKET_SUBPROTOCOL, decode_manager_message,
};
use tokio::{
    net::TcpStream,
    sync::watch,
    time::{Instant, MissedTickBehavior, timeout},
};
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, pem::PemObject},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{StatusCode, header},
        protocol::WebSocketConfig,
    },
};
use zeroize::Zeroizing;

pub const CONNECT_PATH: &str = "/sunshine-client/v1/connect";
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
}

#[derive(Clone, Default)]
pub struct HealthObservation {
    pub sunshine_reachable: bool,
    pub configuration: Option<ConfigSnapshot>,
    pub observed_at: Option<Instant>,
}

/// Endpoint and root must come from protected local provisioning, never from an inbound task.
/// Credentials have no Debug/Serialize implementation and are never put into the URL.
pub struct ManagerConnection {
    endpoint: Url,
    credential: Zeroizing<String>,
    tls: Connector,
}

impl ManagerConnection {
    pub fn new(
        endpoint: &str,
        credential: Zeroizing<String>,
        root_pem: &[u8],
    ) -> Result<Self, TransportError> {
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
        if root_pem.len() > 1024 * 1024 {
            return Err(TransportError::Configuration);
        }
        if root_pem.is_empty() {
            return Ok(Self {
                endpoint,
                credential,
                tls: Connector::NativeTls(
                    native_tls::TlsConnector::builder()
                        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
                        .build()
                        .map_err(|_| TransportError::Configuration)?,
                ),
            });
        }
        let mut roots = rustls::RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(root_pem) {
            roots
                .add(certificate.map_err(|_| TransportError::Configuration)?)
                .map_err(|_| TransportError::Configuration)?;
        }
        if roots.is_empty() {
            return Err(TransportError::Configuration);
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            endpoint,
            credential,
            tls: Connector::Rustls(Arc::new(tls)),
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
            connect_async_tls_with_config(request, Some(config), false, Some(self.tls.clone())),
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

    /// Terminal authentication failure stops reconnecting. Other reconnects use Foundation backoff.
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
                result = self.session(&binding, &capabilities, executor.clone(), &health) => result,
            };
            if matches!(
                result,
                Err(TransportError::Revoked
                    | TransportError::Configuration
                    | TransportError::Protocol)
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
    ) -> Result<(), TransportError> {
        let mut socket = self.connect().await?;
        send(
            &mut socket,
            ClientMessage::Hello {
                binding: binding.clone(),
                capabilities: capabilities.clone(),
            },
        )
        .await?;
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_peer = Instant::now();
        let mut executing: FuturesUnordered<ExecutionFuture> = FuturesUnordered::new();
        let mut in_flight: Option<(String, String)> = None;
        loop {
            tokio::select! {
                // Credential revocation/connection close wins over publishing another result.
                biased;
                incoming = socket.next() => {
                    let frame = incoming.ok_or(TransportError::Disconnected)?.map_err(|_| TransportError::Disconnected)?;
                    last_peer = Instant::now();
                    match frame {
                        Message::Text(text) => match decode_manager_message(text.as_bytes()).map_err(|_| TransportError::Protocol)? {
                            ManagerMessage::Revoked {} => return Err(TransportError::Revoked),
                            ManagerMessage::Task { mode, task } => {
                                let fingerprint = task.fingerprint().map_err(|_| TransportError::Protocol)?;
                                if let Some((id, pending_fingerprint)) = &in_flight {
                                    // A retry can arrive before the first execution finishes. Keep
                                    // the original execution and its eventual result, never run twice.
                                    if id == &task.operation_id && pending_fingerprint == &fingerprint { continue; }
                                    return Err(TransportError::Protocol);
                                }
                                in_flight = Some((task.operation_id.clone(), fingerprint));
                                let executor = executor.clone();
                                executing.push(Box::pin(async move {
                                    let report = executor.deliver(&task, mode).await;
                                    (task.operation_id, report)
                                }));
                            }
                        },
                        Message::Ping(bytes) => send_frame(&mut socket, Message::Pong(bytes)).await?,
                        Message::Pong(_) => {},
                        Message::Close(_) => return Err(TransportError::Disconnected),
                        _ => return Err(TransportError::Protocol),
                    }
                },
                Some((operation_id, report)) = executing.next(), if !executing.is_empty() => {
                    in_flight = None;
                    send(&mut socket, ClientMessage::Result { operation_id, report }).await?;
                },
                _ = tick.tick() => {
                    if last_peer.elapsed() >= PEER_TIMEOUT { return Err(TransportError::Disconnected); }
                    let observation = health.borrow().clone();
                    let fresh = observation.observed_at.is_some_and(|at| at.elapsed() < Duration::from_secs(30));
                    send(&mut socket, ClientMessage::Heartbeat {
                        sunshine_reachable: fresh.then_some(observation.sunshine_reachable),
                        configuration: if fresh { observation.configuration } else { None },
                    }).await?;
                    send_frame(&mut socket, Message::Ping(Vec::new().into())).await?;
                },
            }
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ExecutionFuture = Pin<Box<dyn Future<Output = (String, Report)> + Send>>;

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
        tungstenite::Error::Http(response)
            if matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) =>
        {
            TransportError::Revoked
        }
        _ => TransportError::Disconnected,
    }
}
