//! Explicit local provisioning. No secret command-line arguments or remote installation.
use crate::{
    adapter::{LocalSunshine, Sunshine, VersionSource},
    engine::Executor,
    journal::FileJournal,
    storage::{ProtectedState, StorageError},
    transport::{HealthObservation, ManagerConnection, TransportError},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use sunshine_client_protocol::{
    Binding, Capabilities, ClientOs, Effectiveness, PROTOCOL, SUNSHINE_VERSION,
    is_valid_authorization_code,
};
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub manager_endpoint: String,
    pub enrollment_token: Zeroizing<String>,
    pub sunshine_endpoint: String,
    pub sunshine_username: Zeroizing<String>,
    pub sunshine_password: Zeroizing<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) binding: Binding,
    pub(crate) credential: Zeroizing<String>,
    pub(crate) config: Bootstrap,
    pub(crate) enrolled: bool,
    #[serde(default = "current_sunshine_version")]
    pub(crate) sunshine_version: String,
}

fn current_sunshine_version() -> String {
    SUNSHINE_VERSION.to_owned()
}
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("invalid protected bootstrap configuration")]
    Configuration,
    #[error("invalid Sunshine instance authorization code")]
    InvalidAuthorizationCode,
    #[error("protected state document is malformed")]
    StateDocumentCorrupt { artifact: &'static str },
    #[error("protected state uses an unsupported schema")]
    StateSchemaUnsupported {
        artifact: &'static str,
        detected: &'static str,
        supported: &'static str,
    },
    #[error("Manager enrollment unavailable; retained identity allows a safe retry")]
    Unavailable,
    #[error("device credential or enrollment ticket rejected; local administrator action required")]
    Rejected,
    #[error("Manager rate limited this request; resume the saved transaction later")]
    RateLimited,
    #[error("Manager protocol or platform is unsupported")]
    Unsupported,
    #[error("Manager pairing endpoint was not found")]
    PairingEndpointNotFound,
    #[error("Manager or reverse proxy rejected the pairing HTTP method")]
    PairingHttpMethodRejected,
    #[error("Manager requires a component upgrade")]
    PairingServerUpgradeRequired,
    #[error("Manager rejected the pairing request")]
    PairingRequestRejected,
    #[error("Manager returned an unexpected pairing HTTP status")]
    PairingUnexpectedHttpStatus,
    #[error("Sunshine rejected its local credentials")]
    SunshineCredentialsRejected,
    #[error("Sunshine HTTPS API is unavailable")]
    SunshineApiUnavailable,
    #[error("Sunshine version is unsupported")]
    SunshineVersionUnsupported {
        detected: Option<String>,
        origin: VersionSource,
    },
    #[error("awaiting_pairing: run setup or pair explicitly")]
    Unpaired,
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("Client execution journal unavailable")]
    Journal,
}
fn config_error(_: impl std::fmt::Debug) -> ProvisionError {
    ProvisionError::Configuration
}

fn classify_state_document(bytes: &[u8], artifact: &'static str) -> Result<(), ProvisionError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ProvisionError::StateDocumentCorrupt { artifact })?;
    let object = value
        .as_object()
        .ok_or(ProvisionError::StateDocumentCorrupt { artifact })?;
    let legacy_shape = |candidate: &serde_json::Map<String, serde_json::Value>| {
        candidate.contains_key("sunshine_certificate")
            || candidate.contains_key("restart_allowed")
            || candidate
                .get("manager_endpoint")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|endpoint| endpoint.contains("/sunshine-client/v1/"))
    };
    let legacy = legacy_shape(object)
        || object
            .get("config")
            .and_then(serde_json::Value::as_object)
            .is_some_and(legacy_shape);
    if legacy {
        return Err(ProvisionError::StateSchemaUnsupported {
            artifact,
            detected: "sunshine-client-v1",
            supported: "sunshine-client-v2",
        });
    }
    Err(ProvisionError::StateDocumentCorrupt { artifact })
}

pub(crate) fn decode_bootstrap(bytes: &[u8]) -> Result<Bootstrap, ProvisionError> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_) => {
            classify_state_document(bytes, "bootstrap")?;
            unreachable!()
        }
    }
}

pub(crate) fn decode_identity(bytes: &[u8]) -> Result<Identity, ProvisionError> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_) => {
            classify_state_document(bytes, "identity")?;
            unreachable!()
        }
    }
}
/// Validate locally before accepting a bootstrap; never print its secret fields.
pub(crate) fn validate_bootstrap(bytes: &[u8]) -> Result<(), ProvisionError> {
    let config = decode_bootstrap(bytes)?;
    config.validate()
}
pub(crate) fn pending_retry(config: &[u8], identity: &[u8]) -> bool {
    let Ok(incoming) = decode_bootstrap(config) else {
        return false;
    };
    let Ok(existing) = decode_identity(identity) else {
        return false;
    };
    !existing.enrolled && incoming == existing.config
}

impl Bootstrap {
    pub(crate) fn validate(&self) -> Result<(), ProvisionError> {
        if !is_valid_authorization_code(&self.enrollment_token) {
            return Err(ProvisionError::InvalidAuthorizationCode);
        }
        ManagerConnection::new(&self.manager_endpoint, Zeroizing::new("a".repeat(64)))?;
        self.adapter()?;
        Ok(())
    }
    pub(crate) fn adapter(&self) -> Result<LocalSunshine, ProvisionError> {
        LocalSunshine::new(
            &self.sunshine_endpoint,
            &self.sunshine_username,
            self.sunshine_password.clone(),
        )
        .map_err(config_error)
    }
}
impl Identity {
    pub(crate) fn persist(&self, store: &ProtectedState) -> Result<(), ProvisionError> {
        let bytes = Zeroizing::new(serde_json::to_vec(self).map_err(config_error)?);
        store.put("identity.json", &bytes)?;
        Ok(())
    }
}

fn system_client() -> Result<reqwest::Client, ProvisionError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("sunshine-client/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(config_error)
}

const MAX_MANAGER_RESPONSE_BYTES: usize = 16 * 1024;

struct BoundedResponse {
    status: reqwest::StatusCode,
    body: Vec<u8>,
}

async fn execute_bounded(
    client: &reqwest::Client,
    request: reqwest::Request,
) -> Result<BoundedResponse, ProvisionError> {
    let response = client
        .execute(request)
        .await
        .map_err(|_| ProvisionError::Unavailable)?;
    let header_bytes = response
        .headers()
        .iter()
        .try_fold(0usize, |total, (name, value)| {
            total
                .checked_add(name.as_str().len())?
                .checked_add(value.as_bytes().len())
        });
    if header_bytes.is_none_or(|size| size > MAX_MANAGER_RESPONSE_BYTES)
        || response
            .content_length()
            .is_some_and(|length| length > MAX_MANAGER_RESPONSE_BYTES as u64)
    {
        return Err(ProvisionError::Configuration);
    }
    let status = response.status();
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProvisionError::Unavailable)?
    {
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_MANAGER_RESPONSE_BYTES)
        {
            return Err(ProvisionError::Configuration);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(BoundedResponse { status, body })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingTarget {
    manager_id: Uuid,
    device_id: Uuid,
}
async fn resolve_pairing(config: &Bootstrap) -> Result<PairingTarget, ProvisionError> {
    let mut endpoint = Url::parse(&config.manager_endpoint).map_err(config_error)?;
    endpoint.set_scheme("https").map_err(config_error)?;
    endpoint.set_path("/sunshine-client/v2/pairing");
    let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
    request.headers_mut().insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    *request.body_mut() = Some(
        serde_json::to_vec(
            &serde_json::json!({"authorization_code":config.enrollment_token.as_str()}),
        )
        .map_err(config_error)?
        .into(),
    );
    let client = system_client()?;
    let response = execute_bounded(&client, request).await?;
    if !response.status.is_success() {
        return Err(classify_response(&response));
    }
    let target: PairingTarget = serde_json::from_slice(&response.body).map_err(config_error)?;
    if target.manager_id.is_nil() || target.device_id.is_nil() {
        return Err(ProvisionError::Configuration);
    }
    Ok(target)
}
pub(crate) async fn validate_replacement(
    config: &Bootstrap,
    binding: &Binding,
) -> Result<(), ProvisionError> {
    let target = resolve_pairing(config).await?;
    if target.manager_id != binding.manager_id || target.device_id != binding.device_id {
        return Err(ProvisionError::Configuration);
    }
    Ok(())
}
/// Persist independent random identity before enrollment. Never regenerate on retry.
async fn provision(
    store: &ProtectedState,
    observed_sunshine_version: &str,
) -> Result<Identity, ProvisionError> {
    let mut identity = if let Some(bytes) = store.read("identity.json")? {
        decode_identity(&Zeroizing::new(bytes))?
    } else {
        let bytes = Zeroizing::new(
            store
                .read("bootstrap.json")?
                .ok_or(ProvisionError::Configuration)?,
        );
        let config = decode_bootstrap(&bytes)?;
        config.validate()?;
        let target = resolve_pairing(&config).await?;
        let mut random = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let credential = Zeroizing::new(random.iter().map(|b| format!("{b:02x}")).collect());
        let value = Identity {
            binding: Binding {
                manager_id: target.manager_id,
                device_id: target.device_id,
                installation_id: Uuid::new_v4(),
            },
            credential,
            config,
            enrolled: false,
            sunshine_version: observed_sunshine_version.to_owned(),
        };
        value.persist(store)?;
        value
    };
    if identity.enrolled {
        return Ok(identity);
    }
    let mut endpoint = Url::parse(&identity.config.manager_endpoint).map_err(config_error)?;
    endpoint.set_scheme("https").map_err(config_error)?;
    endpoint.set_path("/sunshine-client/v2/identity");
    let client = system_client()?;
    let mut request = reqwest::Request::new(reqwest::Method::GET, endpoint.clone());
    let raw = Zeroizing::new(format!("Bearer {}", identity.credential.as_str()));
    let mut header = reqwest::header::HeaderValue::from_str(&raw).map_err(config_error)?;
    header.set_sensitive(true);
    request
        .headers_mut()
        .insert(reqwest::header::AUTHORIZATION, header);
    let response = execute_bounded(&client, request).await?;
    let binding: Binding = if response.status.is_success() {
        serde_json::from_slice(&response.body).map_err(config_error)?
    } else if response.status == reqwest::StatusCode::UNAUTHORIZED {
        endpoint.set_path("/sunshine-client/v2/enroll");
        let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
        request.headers_mut().insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        // The body is sent only to the locally pinned TLS endpoint and never logged.
        *request.body_mut()=Some(serde_json::to_vec(&serde_json::json!({"device_id":identity.binding.device_id,"installation_id":identity.binding.installation_id,"token":identity.config.enrollment_token.as_str(),"credential":identity.credential.as_str()})).map_err(config_error)?.into());
        let response = execute_bounded(&client, request).await?;
        if matches!(response.status.as_u16(), 401 | 403) {
            return Err(classify_response(&response));
        }
        if !response.status.is_success() {
            return Err(classify_response(&response));
        }
        serde_json::from_slice(&response.body).map_err(config_error)?
    } else {
        return Err(classify_response(&response));
    };
    if binding != identity.binding {
        return Err(ProvisionError::Configuration);
    }
    identity.enrolled = true;
    identity.sunshine_version = observed_sunshine_version.to_owned();
    identity.config.enrollment_token.clear();
    identity.persist(store)?;
    store.put("bootstrap.json", b"{}")?;
    Ok(identity)
}

/// Local wizard completion requires a verified Sunshine read and successful enrollment.
pub async fn pair(state_path: &Path) -> Result<(), ProvisionError> {
    client_os()?;
    let _root = crate::storage::prepare_root(state_path)?;
    let store = ProtectedState::open(&state_path.join("provisioning"))?;
    let config: Bootstrap = if let Some(bytes) = store.read("identity.json")? {
        decode_identity(&Zeroizing::new(bytes))?.config
    } else {
        decode_bootstrap(&Zeroizing::new(
            store
                .read("bootstrap.json")?
                .ok_or(ProvisionError::Configuration)?,
        ))?
    };
    let sunshine_version = config
        .adapter()?
        .read()
        .await
        .map_err(|error| match error {
            crate::adapter::AdapterError::CredentialsRejected => {
                ProvisionError::SunshineCredentialsRejected
            }
            crate::adapter::AdapterError::ApiUnavailable => ProvisionError::SunshineApiUnavailable,
            crate::adapter::AdapterError::UnsupportedVersion { detected, origin } => {
                ProvisionError::SunshineVersionUnsupported { detected, origin }
            }
            crate::adapter::AdapterError::UnsafeConfiguration
            | crate::adapter::AdapterError::InvalidLocalEndpoint
            | crate::adapter::AdapterError::ResourceConflict
            | crate::adapter::AdapterError::UnsupportedCapability => ProvisionError::Configuration,
        })?
        .sunshine_version()
        .to_owned();
    provision(&store, &sunshine_version).await?;
    Ok(())
}
pub async fn run(state_path: &Path, shutdown: watch::Receiver<bool>) -> Result<(), ProvisionError> {
    let _maintenance = crate::storage::MaintenanceGuard::acquire(state_path)?;
    let _root = crate::storage::prepare_root(state_path)?;
    let store = ProtectedState::open(&state_path.join("provisioning"))?;
    // Service startup never enrolls, resumes pairing, or rotates credentials.
    let bytes = Zeroizing::new(
        store
            .read("identity.json")?
            .ok_or(ProvisionError::Unpaired)?,
    );
    let identity = decode_identity(&bytes)?;
    if !identity.enrolled {
        return Err(ProvisionError::Unpaired);
    }
    let _status = crate::runtime_status::publish(
        state_path,
        identity.binding.installation_id.to_string(),
        crate::cli::settings_revision(&identity.config).map_err(config_error)?,
    )
    .map_err(|_| ProvisionError::Storage(StorageError::Unsafe))?;
    drop(store); // Do not retain the short provisioning lock across network waits.
    let journal =
        FileJournal::open(&state_path.join("journal")).map_err(|_| ProvisionError::Journal)?;
    let capabilities = Capabilities {
        protocol: PROTOCOL.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        os: client_os()?,
        sunshine_version: identity.sunshine_version.clone(),
        restart_allowed: true,
        managed_fields: sunshine_client_protocol::config::FIELD_DEFINITIONS
            .iter()
            .map(|field| field.key.to_owned())
            .collect(),
        application_management: true,
        application_host_commands_allowed: true,
        moonlight_pairing_management: true,
        diagnostics: true,
        maintenance: true,
        service_control: crate::adapter::platform_service_control_available(),
    };
    let executor = Arc::new(Executor::new_with_capabilities(
        identity.binding.clone(),
        capabilities.clone(),
        identity.config.adapter()?,
        journal,
    ));
    let connection =
        ManagerConnection::new(&identity.config.manager_endpoint, identity.credential)?;
    let (health_tx, health_rx) = watch::channel(HealthObservation::default());
    let mut adapter = identity.config.adapter()?;
    // Read-only health probes do not share the mutation lane and survive a Sunshine restart.
    let monitor = async move {
        loop {
            let started = tokio::time::Instant::now();
            let read = adapter.read().await;
            crate::runtime_status::observe("sunshine_reachable", serde_json::json!(read.is_ok()));
            crate::runtime_status::observe(
                "sunshine_observed_at",
                serde_json::json!(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                ),
            );
            let observation = HealthObservation {
                sunshine_reachable: read.is_ok(),
                configuration: read
                    .ok()
                    .map(|c| c.snapshot(Effectiveness::PendingVerification)),
                observed_at: Some(started),
            };
            if health_tx.send(observation).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    };
    let result = tokio::select! {
        result=connection.run(identity.binding,capabilities,executor,health_rx,shutdown)=>result.map_err(ProvisionError::from),
        _=monitor=>Err(ProvisionError::Unavailable),
    };
    if let Err(error) = &result {
        let code = match error {
            ProvisionError::Transport(TransportError::Revoked) | ProvisionError::Rejected => {
                "credential_rejected"
            }
            ProvisionError::Transport(TransportError::Protocol) | ProvisionError::Unsupported => {
                "unsupported_protocol_or_platform"
            }
            ProvisionError::SunshineCredentialsRejected => "sunshine_credentials_rejected",
            ProvisionError::SunshineVersionUnsupported { .. } => "sunshine_version_unsupported",
            _ => "runtime_failure",
        };
        crate::runtime_status::observe("last_error_code", serde_json::json!(code));
    }
    result
}

#[cfg(test)]
mod bootstrap_tests {
    use super::ProvisionError;
    fn configuration() -> serde_json::Value {
        serde_json::json!({"manager_endpoint":"wss://manager.example.org/sunshine-client/v2/connect", "enrollment_token":"a".repeat(sunshine_client_protocol::AUTHORIZATION_CODE_LENGTH),
            "sunshine_endpoint":"https://127.0.0.1:47990/", "sunshine_username":"fixture", "sunshine_password":"local-only"})
    }
    #[test]
    fn bootstrap_accepts_only_loopback_sunshine_and_server_resolved_binding() {
        let config = configuration();
        assert!(super::validate_bootstrap(&serde_json::to_vec(&config).unwrap()).is_ok());
        for field in [
            "manager_ca_pem",
            "sunshine_ca_pem",
            "sunshine_certificate",
            "sunshine_certificate_path",
            "manager_id",
            "device_id",
            "restart_allowed",
            "application_host_commands_allowed",
            "service_control_mode",
        ] {
            let mut extra = config.clone();
            extra[field] = serde_json::json!("not permitted");
            assert!(super::validate_bootstrap(&serde_json::to_vec(&extra).unwrap()).is_err());
        }
    }

    #[test]
    fn bootstrap_uses_the_manager_authorization_code_contract() {
        let mut config = configuration();
        assert!(super::validate_bootstrap(&serde_json::to_vec(&config).unwrap()).is_ok());

        for invalid in ["a".repeat(35), "a".repeat(64), "A".repeat(36)] {
            config["enrollment_token"] = serde_json::json!(invalid);
            assert!(matches!(
                super::validate_bootstrap(&serde_json::to_vec(&config).unwrap()),
                Err(ProvisionError::InvalidAuthorizationCode)
            ));
        }
    }

    #[test]
    fn legacy_bootstrap_is_rejected_as_unsupported_without_migration() {
        let legacy = serde_json::json!({
            "manager_endpoint": "wss://manager.example.org/sunshine-client/v1/connect",
            "enrollment_token": "a".repeat(sunshine_client_protocol::AUTHORIZATION_CODE_LENGTH),
            "sunshine_endpoint": "https://127.0.0.1:47990/",
            "sunshine_username": "fixture",
            "sunshine_password": "secret",
            "sunshine_certificate": "legacy",
            "restart_allowed": true
        });
        assert!(matches!(
            super::decode_bootstrap(&serde_json::to_vec(&legacy).unwrap()),
            Err(ProvisionError::StateSchemaUnsupported {
                artifact: "bootstrap",
                detected: "sunshine-client-v1",
                supported: "sunshine-client-v2"
            })
        ));
    }

    #[test]
    fn malformed_bootstrap_is_not_misreported_as_an_unsafe_acl() {
        assert!(matches!(
            super::decode_bootstrap(br#"{"manager_endpoint":"#),
            Err(ProvisionError::StateDocumentCorrupt {
                artifact: "bootstrap"
            })
        ));
    }
    #[test]
    fn retry_preserves_pending_identity_and_refuses_changed_or_enrolled_configuration() {
        let config = configuration();
        let bytes = serde_json::to_vec(&config).unwrap();
        let mut identity = serde_json::json!({"binding":{"manager_id":uuid::Uuid::new_v4(),"device_id":uuid::Uuid::new_v4(),"installation_id":uuid::Uuid::new_v4()},"credential":"b".repeat(64),"config":config,"enrolled":false});
        assert!(super::pending_retry(
            &bytes,
            &serde_json::to_vec(&identity).unwrap()
        ));
        identity["config"]["enrollment_token"] =
            serde_json::json!("c".repeat(sunshine_client_protocol::AUTHORIZATION_CODE_LENGTH));
        assert!(!super::pending_retry(
            &bytes,
            &serde_json::to_vec(&identity).unwrap()
        ));
        identity["config"] = configuration();
        identity["enrolled"] = serde_json::json!(true);
        assert!(!super::pending_retry(
            &bytes,
            &serde_json::to_vec(&identity).unwrap()
        ));
    }
    #[test]
    fn incomplete_or_example_bootstrap_is_not_accepted() {
        for bytes in [
            b"{}".as_slice(),
            b"not JSON".as_slice(),
            include_bytes!("../deploy/bootstrap.example.json").as_slice(),
        ] {
            assert!(super::validate_bootstrap(bytes).is_err());
        }
    }
}

fn classify_response(response: &BoundedResponse) -> ProvisionError {
    #[derive(Deserialize)]
    struct ErrorCode {
        code: String,
    }
    if serde_json::from_slice::<ErrorCode>(&response.body)
        .is_ok_and(|error| error.code == "unsupported_client_protocol")
    {
        return ProvisionError::Unsupported;
    }
    let status = response.status.as_u16();
    match status {
        401 | 403 => ProvisionError::Rejected,
        429 => ProvisionError::RateLimited,
        408 | 500..=599 => ProvisionError::Unavailable,
        404 => ProvisionError::PairingEndpointNotFound,
        405 => ProvisionError::PairingHttpMethodRejected,
        406 | 426 => ProvisionError::PairingServerUpgradeRequired,
        400..=499 => ProvisionError::PairingRequestRejected,
        _ => ProvisionError::PairingUnexpectedHttpStatus,
    }
}
fn client_os() -> Result<ClientOs, ProvisionError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok(ClientOs::WindowsX86_64),
        ("linux", "x86_64") => Ok(ClientOs::LinuxX86_64),
        ("macos", "x86_64") => Ok(ClientOs::MacosX86_64),
        ("macos", "aarch64") => Ok(ClientOs::MacosAarch64),
        _ => Err(ProvisionError::Unsupported),
    }
}
#[cfg(test)]
mod error_tests {
    use super::*;
    #[test]
    fn temporary_failures_are_not_bad_credentials() {
        let response = |status, body: &[u8]| BoundedResponse {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            body: body.to_vec(),
        };
        assert!(matches!(
            classify_response(&response(503, b"")),
            ProvisionError::Unavailable
        ));
        assert!(matches!(
            classify_response(&response(404, b"")),
            ProvisionError::PairingEndpointNotFound
        ));
        assert!(matches!(
            classify_response(&response(405, b"")),
            ProvisionError::PairingHttpMethodRejected
        ));
        assert!(matches!(
            classify_response(&response(426, b"")),
            ProvisionError::PairingServerUpgradeRequired
        ));
        assert!(matches!(
            classify_response(&response(400, br#"{"code":"unsupported_client_protocol"}"#)),
            ProvisionError::Unsupported
        ));
        assert!(matches!(
            classify_response(&response(
                400,
                br#"{"code":"anything_else","message":"secret"}"#
            )),
            ProvisionError::PairingRequestRejected
        ));
    }
}

pub(crate) async fn network_probe(config: &Bootstrap) -> Result<serde_json::Value, ProvisionError> {
    let mut url = Url::parse(&config.manager_endpoint).map_err(config_error)?;
    url.set_scheme("https").map_err(config_error)?;
    url.set_path("/healthz");
    let client = system_client()?;
    let request = client.get(url).build().map_err(config_error)?;
    let response = execute_bounded(&client, request).await?;
    if !response.status.is_success() {
        return Err(classify_response(&response));
    }
    Ok(
        serde_json::json!({"reachable":true,"scope":"public_health_endpoint","trust_context":"current_cli_account"}),
    )
}
