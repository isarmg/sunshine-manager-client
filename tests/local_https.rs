//! Real HTTPS and HTTP behavior against a local protocol fixture, NOT Sunshine acceptance.
use std::{
    path::Path,
    process::Command as ProcessCommand,
    sync::{Arc, Mutex},
};
use sunshine_client::{
    adapter::{AdapterError, LocalSunshine, Sunshine},
    engine::*,
};
use sunshine_client_protocol::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};
use zeroize::Zeroizing;

fn openssl(path: &Path, args: &[&str]) {
    let output = ProcessCommand::new("openssl")
        .current_dir(path)
        .args(args)
        .output()
        .expect("openssl is a required test dependency");
    assert!(
        output.status.success(),
        "test certificate generation failed"
    );
}

fn certificates(path: &Path) {
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Client Test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ],
    );
    // Mirrors Sunshine's built-in certificate: self-signed CN, without a loopback SAN.
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "sunshine.key",
            "-out",
            "sunshine.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Sunshine Gamestream Host",
        ],
    );
    openssl(
        path,
        &[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=IP:127.0.0.1",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-addext",
            "extendedKeyUsage=serverAuth",
        ],
    );
    openssl(
        path,
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.pem",
            "-days",
            "1",
            "-copy_extensions",
            "copy",
        ],
    );
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "wrong.key",
            "-out",
            "wrong.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Untrusted Test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ],
    );
}

struct Fixture {
    endpoint: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(path: &Path, responses: Vec<String>) -> Fixture {
    let cert = CertificateDer::from_pem_file(path.join("sunshine.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_file(path.join("sunshine.key")).unwrap();
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        let mut responses = responses.into_iter();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let Ok(mut tls) = acceptor.accept(socket).await else {
                continue;
            };
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = match tls.read(&mut buffer).await {
                    Ok(count) => count,
                    Err(_) => {
                        // Schannel can reject the certificate after the server's handshake future
                        // completed. A TLS abort must not kill the fixture or consume an HTTP reply.
                        bytes.clear();
                        break;
                    }
                };
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                assert!(bytes.len() < 1024 * 1024);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .map(|length| length.parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            if bytes.is_empty() {
                continue;
            }
            captured.lock().unwrap().push(bytes);
            let response = responses.next().unwrap_or_else(|| "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
            let _ = tls.write_all(response.as_bytes()).await;
            let _ = tls.shutdown().await;
        }
    });
    Fixture {
        endpoint,
        requests,
        server,
    }
}

fn response(value: serde_json::Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn text_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn config(qp: &str) -> serde_json::Value {
    serde_json::json!({
        "status": true, "platform": "linux", "version": SUNSHINE_VERSION, "qp": qp,
        "file_apps": "/do-not-expose/apps.json", "unmanaged": "preserve"
    })
}

fn adapter(fixture: &Fixture) -> LocalSunshine {
    LocalSunshine::new(
        &fixture.endpoint,
        "fixture",
        Zeroizing::new("local-test-password".into()),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires loopback sockets and openssl"]
async fn self_signed_loopback_https_is_accepted_and_redirects_are_never_followed() {
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let fixture = serve(temporary.path(), vec![response(config("28"))]).await;
    assert!(adapter(&fixture).read().await.is_ok());
    {
        let captured = fixture.requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let headers = String::from_utf8_lossy(&captured[0]).to_ascii_lowercase();
        assert!(headers.starts_with("get /api/config "));
        assert!(headers.contains("authorization: basic "));
        assert!(!headers.contains("origin:"));
        assert!(!headers.contains("referer:"));
    }
    let redirect = serve(temporary.path(), vec![format!("HTTP/1.1 302 Found\r\nLocation: {}/api/config\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", fixture.endpoint)]).await;
    assert!(adapter(&redirect).read().await.is_err());
    assert_eq!(
        fixture.requests.lock().unwrap().len(),
        1,
        "redirect target must not receive credentials"
    );
    assert_eq!(redirect.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires loopback sockets and openssl"]
async fn malformed_success_and_oversized_body_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let fixture = serve(
        temporary.path(),
        vec![
            response(serde_json::json!({ "unexpected": true })),
            response(serde_json::json!({"status": true, "huge": "x".repeat(600 * 1024)})),
        ],
    )
    .await;
    assert!(adapter(&fixture).read().await.is_err());
    assert!(adapter(&fixture).read().await.is_err());
}

#[tokio::test]
#[ignore = "requires loopback sockets and openssl"]
async fn protocol_v2_uses_fixed_sunshine_resources_and_reconciles_application_reordering() {
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let original = serde_json::json!({"name":"Steam","output":"","cmd":"","working-dir":"","exclude-global-prep-cmd":false,"elevated":false,"auto-detach":false,"wait-all":false,"exit-timeout":5,"prep-cmd":[],"detached":[],"image-path":""});
    let updated = serde_json::json!({"name":"Steam Remote","output":"","cmd":"","working-dir":"","exclude-global-prep-cmd":false,"elevated":false,"auto-detach":false,"wait-all":false,"exit-timeout":5,"prep-cmd":[],"detached":[],"image-path":""});
    let client_uuid = "123e4567-e89b-12d3-a456-426614174000";
    let fixture = serve(
        temporary.path(),
        vec![
            response(serde_json::json!({"apps":[original.clone()],"env":{}})),
            response(serde_json::json!({"apps":[original],"env":{}})),
            response(serde_json::json!({"status":true})),
            response(serde_json::json!({"apps":[updated],"env":{}})),
            response(serde_json::json!({"status":true,"named_certs":[{"uuid":client_uuid,"name":"TV","enabled":true}]})),
            text_response("ready\npassword=secret\n"),
            response(serde_json::json!({"virtualhid":{"installed":false,"version":"","minimum_version":"1.0"},"vigembus":{"installed":true,"version":"1.2","minimum_version":"1.0"}})),
            response(serde_json::json!({"status":true})),
            response(serde_json::json!({"status":true})),
        ],
    )
    .await;
    let mut sunshine = adapter(&fixture);
    let applications = sunshine.applications().await.unwrap();
    let target = applications.applications[0].reference.clone();
    let replacement = ApplicationSpec {
        name: "Steam Remote".into(),
        output: String::new(),
        cmd: String::new(),
        working_dir: String::new(),
        exclude_global_prep_cmd: false,
        elevated: false,
        auto_detach: false,
        wait_all: false,
        exit_timeout: 5,
        prep_cmd: vec![],
        detached: vec![],
        image_path: String::new(),
    };
    let saved = sunshine
        .save_application(&applications.revision, Some(&target), &replacement)
        .await
        .unwrap();
    assert_eq!(saved.applications[0].specification.name, "Steam Remote");
    assert_eq!(
        sunshine.paired_clients().await.unwrap().clients[0].uuid,
        client_uuid
    );
    let logs = sunshine.logs(None, 4096).await.unwrap();
    assert!(logs.text.contains("ready"));
    assert!(!logs.text.contains("secret"));
    assert!(logs.redacted);
    let drivers = sunshine.virtual_input_status().await.unwrap();
    assert!(drivers.vigembus.installed);
    sunshine
        .maintenance(MaintenanceAction::ResetDisplayPersistence)
        .await
        .unwrap();
    sunshine
        .submit_pairing_pin("0123456789abcdef0123456789abcdef", "1234", "TV")
        .await
        .unwrap();
    let requests = fixture.requests.lock().unwrap();
    let requests = requests
        .iter()
        .map(|value| String::from_utf8_lossy(value).to_string())
        .collect::<Vec<_>>();
    assert!(requests[2].starts_with("POST /api/apps "));
    assert!(requests[2].contains("\"index\":0"));
    assert!(requests[5].starts_with("GET /api/logs "));
    assert!(requests[7].starts_with("POST /api/reset-display-device-persistence "));
    assert!(requests[8].starts_with("POST /api/pin "));
    assert!(
        requests
            .iter()
            .all(|request| !request.to_ascii_lowercase().contains("origin:"))
    );
}

#[tokio::test]
#[ignore = "requires loopback sockets and openssl"]
async fn sunshine_http_credentials_api_and_version_failures_are_distinct() {
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let fixture = serve(
        temporary.path(),
        vec![
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .into(),
            response(serde_json::json!({
                "status": true,
                "platform": "windows",
                "version": "999.0.0"
            })),
        ],
    )
    .await;
    assert!(matches!(
        adapter(&fixture).read().await,
        Err(AdapterError::CredentialsRejected)
    ));
    assert!(matches!(
        adapter(&fixture).read().await,
        Err(AdapterError::ApiUnavailable)
    ));
    assert!(matches!(
        adapter(&fixture).read().await,
        Err(AdapterError::UnsupportedVersion { .. })
    ));
}

#[derive(Default)]
struct JournalFixture(std::collections::BTreeMap<String, ExecutionRecord>);
impl Journal for JournalFixture {
    fn load(&mut self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        Ok(self.0.get(id).cloned())
    }
    fn create(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.0.insert(id.into(), record.clone());
        Ok(())
    }
    fn replace(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.create(id, record)
    }
}

#[tokio::test]
#[ignore = "requires loopback sockets and openssl"]
async fn real_https_patch_preserves_original_configuration_and_filters_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let fixture = serve(
        temporary.path(),
        vec![
            response(config("28")),
            response(config("28")),
            response(serde_json::json!({"status": true})),
            response(config("29")),
        ],
    )
    .await;
    let binding = Binding {
        manager_id: uuid::Uuid::from_u128(1),
        device_id: uuid::Uuid::from_u128(2),
        installation_id: uuid::Uuid::from_u128(3),
    };
    let executor = Executor::new(
        binding.clone(),
        adapter(&fixture),
        JournalFixture::default(),
    );
    let task = Task {
        protocol: PROTOCOL.into(),
        operation_id: format!("op_{}", uuid::Uuid::new_v4()),
        binding,
        permission: Permission::WriteConfig,
        command: Command::PatchConfig {
            expected_revision: sunshine_client::adapter::Configuration::from_response(config("28"))
                .unwrap()
                .revision(),
            set: std::collections::BTreeMap::from([(
                "qp".into(),
                sunshine_client_protocol::config::FieldValue::Integer(29),
            )]),
            remove: Default::default(),
            restart_policy: RestartPolicy::Manual,
        },
    };
    assert!(matches!(
        executor.deliver(&task, DeliveryMode::Execute).await,
        Report::ConfigSaved { .. }
    ));
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let saved = String::from_utf8_lossy(&requests[2]);
    assert!(saved.starts_with("POST /api/config "));
    let body: serde_json::Value =
        serde_json::from_str(saved.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["file_apps"], "/do-not-expose/apps.json");
    assert_eq!(body["unmanaged"], "preserve");
    assert_eq!(body["qp"], "29");
    for metadata in ["status", "version", "platform"] {
        assert!(body.get(metadata).is_none());
    }
}

#[test]
fn credentials_are_not_formatted_in_adapter_errors() {
    assert_eq!(
        AdapterError::ApiUnavailable.to_string(),
        "Sunshine HTTPS API is unavailable"
    );
}

#[tokio::test]
#[ignore = "requires a Manager certificate trusted by the standard WebPKI roots"]
async fn authenticated_wss_delivers_result_and_revocation_stops_reconnect() {
    use futures_util::{SinkExt, StreamExt};
    use sunshine_client::transport::{
        CONNECT_PATH, HealthObservation, ManagerConnection, TransportError,
    };
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            Message,
            handshake::server::{Request, Response},
            http::header,
        },
    };
    let temporary = tempfile::tempdir().unwrap();
    certificates(temporary.path());
    let sunshine = serve(temporary.path(), vec![response(config("28"))]).await;
    let binding = Binding {
        manager_id: uuid::Uuid::from_u128(1),
        device_id: uuid::Uuid::from_u128(2),
        installation_id: uuid::Uuid::from_u128(3),
    };
    let executor = Arc::new(Executor::new(
        binding.clone(),
        adapter(&sunshine),
        JournalFixture::default(),
    ));
    let cert = CertificateDer::from_pem_file(temporary.path().join("server.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_file(temporary.path().join("server.key")).unwrap();
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("wss://{}{CONNECT_PATH}", listener.local_addr().unwrap());
    let task = Task {
        protocol: PROTOCOL.into(),
        operation_id: format!("op_{}", uuid::Uuid::new_v4()),
        binding: binding.clone(),
        permission: Permission::ReadConfig,
        command: sunshine_client_protocol::Command::ReadConfig {},
    };
    let manager = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        #[allow(
            clippy::result_large_err,
            reason = "tungstenite fixes the callback error type to an HTTP response"
        )]
        let mut socket = accept_hdr_async(tls, |request: &Request, mut response: Response| {
            assert_eq!(request.uri().path(), CONNECT_PATH);
            assert!(
                request.headers().get(header::AUTHORIZATION).unwrap()
                    == format!("Bearer {}", "a".repeat(64)).as_str()
            );
            assert_eq!(
                request
                    .headers()
                    .get(header::SEC_WEBSOCKET_PROTOCOL)
                    .unwrap(),
                WEBSOCKET_SUBPROTOCOL
            );
            response.headers_mut().insert(
                header::SEC_WEBSOCKET_PROTOCOL,
                header::HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
            );
            Ok(response)
        })
        .await
        .unwrap();
        let Message::Text(hello) = socket.next().await.unwrap().unwrap() else {
            panic!("missing hello")
        };
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(&hello).unwrap(),
            ClientMessage::Hello { .. }
        ));
        socket
            .send(Message::Text(
                serde_json::to_string(&ManagerMessage::Task {
                    mode: DeliveryMode::Execute,
                    task: Box::new(task.clone()),
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        // Repeat before the first result: the transport must preserve the active operation.
        socket
            .send(Message::Text(
                serde_json::to_string(&ManagerMessage::Task {
                    mode: DeliveryMode::Execute,
                    task: Box::new(task.clone()),
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let mut got_result = false;
        let mut got_health = false;
        while !got_result || !got_health {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(message) => {
                    match serde_json::from_str::<ClientMessage>(&message).unwrap() {
                        ClientMessage::Result {
                            operation_id,
                            report,
                        } => {
                            assert_eq!(operation_id, task.operation_id);
                            assert!(matches!(report, Report::ConfigRead { .. }));
                            got_result = true;
                        }
                        ClientMessage::Heartbeat {
                            sunshine_reachable,
                            configuration,
                        } => {
                            assert_eq!(
                                sunshine_reachable, None,
                                "an expired probe must not report Sunshine online"
                            );
                            assert!(configuration.is_none());
                            got_health = true;
                        }
                        _ => panic!("unexpected hello"),
                    }
                }
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                _ => {}
            }
        }
        socket
            .send(Message::Text(
                serde_json::to_string(&ManagerMessage::Revoked {})
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    });
    let connection = ManagerConnection::new(&endpoint, Zeroizing::new("a".repeat(64))).unwrap();
    let (_health_tx, health) = tokio::sync::watch::channel(HealthObservation {
        sunshine_reachable: true,
        configuration: None,
        observed_at: Some(tokio::time::Instant::now() - std::time::Duration::from_secs(60)),
    });
    let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
    let capabilities = Capabilities {
        protocol: PROTOCOL.into(),
        client_version: "test".into(),
        os: ClientOs::LinuxX86_64,
        sunshine_version: SUNSHINE_VERSION.into(),
        restart_allowed: false,
        managed_fields: sunshine_client_protocol::config::FIELD_DEFINITIONS
            .iter()
            .map(|field| field.key.into())
            .collect(),
        application_management: true,
        application_host_commands_allowed: false,
        moonlight_pairing_management: true,
        diagnostics: true,
        maintenance: true,
        service_control: false,
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        connection.run(binding, capabilities, executor, health, shutdown),
    )
    .await
    .unwrap();
    assert_eq!(result, Err(TransportError::Revoked));
    manager.await.unwrap();
    assert_eq!(sunshine.requests.lock().unwrap().len(), 1);
}

#[test]
fn wss_endpoint_policy_rejects_plaintext_and_credentials_in_urls() {
    use sunshine_client::transport::validate_manager_endpoint;
    use url::Url;
    for endpoint in [
        "ws://manager.example/sunshine-client/v2/connect",
        "https://manager.example/sunshine-client/v2/connect",
        "wss://token@manager.example/sunshine-client/v2/connect",
        "wss://manager.example/sunshine-client/v2/connect?token=secret",
        "wss://manager.example/other",
    ] {
        assert!(validate_manager_endpoint(&Url::parse(endpoint).unwrap()).is_err());
    }
    assert!(
        validate_manager_endpoint(
            &Url::parse("wss://manager.example/sunshine-client/v2/connect").unwrap()
        )
        .is_ok()
    );
}
