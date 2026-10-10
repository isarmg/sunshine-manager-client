use std::{collections::BTreeMap, process::Output, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Method, Request, header, redirect::Policy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_rustls::rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{
        WebPkiSupportedAlgorithms, aws_lc_rs, verify_tls12_signature, verify_tls13_signature,
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use url::Url;
use xscs_protocol::{
    ApplicationRef, ApplicationSpec, ApplicationView, ApplicationsSnapshot, ConfigSnapshot,
    DriverStatus, Effectiveness, LogCursor, LogPage, MaintenanceAction, PairedClient,
    PairedClientsSnapshot, PendingPairing, ServiceAction, ServiceState, VirtualInputStatus,
};
use zeroize::Zeroizing;

pub const MAX_CONFIG_BYTES: usize = 512 * 1024;

/// Never print upstream errors/bodies: they may include local settings or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    ConfigurationResponse,
    HttpUpgradeRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("Sunshine rejected the configured local credentials")]
    CredentialsRejected,
    #[error("Sunshine HTTPS API is unavailable")]
    ApiUnavailable,
    #[error("unsupported Sunshine version")]
    UnsupportedVersion {
        detected: Option<String>,
        origin: VersionSource,
    },
    #[error("unsafe or malformed Sunshine configuration")]
    UnsafeConfiguration,
    #[error("invalid local HTTPS configuration")]
    InvalidLocalEndpoint,
    #[error("Sunshine resource changed before the operation could be applied")]
    ResourceConflict,
    #[error("the locally selected adapter does not support this capability")]
    UnsupportedCapability,
    #[error("Sunshine did not complete the Moonlight pairing handshake")]
    PairingRejected,
}

fn bounded_version_diagnostic(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_graphic()))
        .then(|| value.to_owned())
}

/// Full configuration stays on the Client. Deliberately not Debug or Serialize.
#[derive(Clone, PartialEq, Eq)]
pub struct Configuration {
    sunshine_version: String,
    platform: String,
    fields: BTreeMap<String, String>,
}

impl Configuration {
    pub fn from_response(response: Value) -> Result<Self, AdapterError> {
        let mut response = response
            .as_object()
            .cloned()
            .ok_or(AdapterError::UnsafeConfiguration)?;
        if response.remove("status") != Some(Value::Bool(true)) {
            return Err(AdapterError::ApiUnavailable);
        }
        let sunshine_version = response
            .remove("version")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or(AdapterError::UnsafeConfiguration)?;
        if !xscs_protocol::is_supported_sunshine_version(&sunshine_version) {
            return Err(AdapterError::UnsupportedVersion {
                detected: bounded_version_diagnostic(&sunshine_version),
                origin: VersionSource::ConfigurationResponse,
            });
        }
        let platform = response
            .remove("platform")
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| !value.is_empty() && value.len() <= 64)
            .ok_or(AdapterError::UnsafeConfiguration)?;
        let mut fields = BTreeMap::new();
        for (key, value) in response {
            // Metadata not in the supported successful response must not be written as a setting.
            if matches!(key.as_str(), "error" | "status_code")
                || key.is_empty()
                || key.len() > 128
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                return Err(AdapterError::UnsafeConfiguration);
            }
            let value = value.as_str().ok_or(AdapterError::UnsafeConfiguration)?;
            if value.len() > 64 * 1024
                || value.contains('\0')
                || (xscs_protocol::config::contains_field(&key) && value.len() > 512)
            {
                return Err(AdapterError::UnsafeConfiguration);
            }
            // Sunshine's writer omits empty values; normalize these before revision calculation.
            if !value.is_empty() {
                fields.insert(key, value.to_owned());
            }
        }
        let configuration = Self {
            sunshine_version,
            platform,
            fields,
        };
        configuration.encoded()?;
        Ok(configuration)
    }

    pub fn revision(&self) -> String {
        // BTreeMap gives a canonical ordering independent of Sunshine's hash-map iteration.
        // Length prefixes avoid delimiter ambiguity, including embedded newlines in local lists.
        let mut digest = Sha256::new();
        digest.update(b"sunshine-config-v1\0");
        for (key, value) in &self.fields {
            digest.update((key.len() as u64).to_be_bytes());
            digest.update(key.as_bytes());
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn sunshine_version(&self) -> &str {
        &self.sunshine_version
    }
    pub fn platform(&self) -> &str {
        &self.platform
    }

    pub fn snapshot(&self, effectiveness: Effectiveness) -> ConfigSnapshot {
        ConfigSnapshot {
            revision: self.revision(),
            sunshine_version: self.sunshine_version.clone(),
            fields: self
                .fields
                .iter()
                .filter_map(|(key, value)| {
                    xscs_protocol::config::normalized_snapshot_value(key, value)
                        .filter(|value| {
                            xscs_protocol::config::validate_snapshot_field(key, value).is_ok()
                        })
                        .map(|value| (key.clone(), value))
                })
                .collect(),
            effectiveness,
        }
    }

    pub(crate) fn merge(
        &self,
        set: &BTreeMap<String, xscs_protocol::config::FieldValue>,
        remove: &std::collections::BTreeSet<String>,
    ) -> Result<Self, AdapterError> {
        let mut merged = self.clone();
        for (key, value) in set {
            xscs_protocol::config::validate_field(key, value)
                .map_err(|_| AdapterError::UnsafeConfiguration)?;
            if remove.contains(key) {
                return Err(AdapterError::UnsafeConfiguration);
            }
            merged.fields.insert(key.clone(), value.sunshine_text());
        }
        for key in remove {
            if !xscs_protocol::config::contains_field(key) {
                return Err(AdapterError::UnsafeConfiguration);
            }
            merged.fields.remove(key);
        }
        merged.encoded()?;
        Ok(merged)
    }

    fn encoded(&self) -> Result<Vec<u8>, AdapterError> {
        let bytes =
            serde_json::to_vec(&self.fields).map_err(|_| AdapterError::UnsafeConfiguration)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(AdapterError::UnsafeConfiguration);
        }
        Ok(bytes)
    }
}

#[async_trait]
pub trait Sunshine: Send {
    fn set_execution_deadline(&mut self, _deadline: Option<crate::engine::ExecutionDeadline>) {}
    async fn save_overwrite(
        &mut self,
        _before: &Configuration,
        configuration: &Configuration,
    ) -> Result<(), AdapterError> {
        self.save(configuration).await
    }
    async fn save_guarded(
        &mut self,
        expected: &str,
        configuration: &Configuration,
    ) -> Result<(), AdapterError> {
        if self.read().await?.revision() != expected {
            return Err(AdapterError::ResourceConflict);
        }
        self.save(configuration).await
    }
    async fn read(&mut self) -> Result<Configuration, AdapterError>;
    async fn save(&mut self, configuration: &Configuration) -> Result<(), AdapterError>;
    async fn restart(&mut self) -> Result<(), AdapterError>;
    /// Actual Sunshine PID and birth identity, never the service-wrapper state.
    async fn process_generation(&mut self) -> Option<String> {
        None
    }
    async fn applications(&mut self) -> Result<ApplicationsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn save_application(
        &mut self,
        _expected: &str,
        _target: Option<&ApplicationRef>,
        _application: &ApplicationSpec,
    ) -> Result<ApplicationsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn delete_application(
        &mut self,
        _expected: &str,
        _target: &ApplicationRef,
    ) -> Result<ApplicationsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn close_application(&mut self) -> Result<(), AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn upload_cover(&mut self, _key: &str, _png: &[u8]) -> Result<String, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn submit_pairing_pin(
        &mut self,
        _pairing_id: &str,
        _pin: &str,
        _name: &str,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }

    async fn pending_pairings(&mut self) -> Result<Vec<PendingPairing>, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn paired_clients(&mut self) -> Result<PairedClientsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn set_paired_client_enabled(
        &mut self,
        _uuid: &str,
        _enabled: bool,
    ) -> Result<PairedClientsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn unpair_client(&mut self, _uuid: &str) -> Result<PairedClientsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn unpair_all_clients(&mut self) -> Result<PairedClientsSnapshot, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn logs(
        &mut self,
        _cursor: Option<&LogCursor>,
        _limit: u32,
    ) -> Result<LogPage, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn virtual_input_status(&mut self) -> Result<VirtualInputStatus, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn maintenance(&mut self, _action: MaintenanceAction) -> Result<(), AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
    async fn service_status(&mut self) -> Result<ServiceState, AdapterError> {
        Ok(ServiceState::Unavailable)
    }
    async fn control_service(
        &mut self,
        _action: ServiceAction,
    ) -> Result<ServiceState, AdapterError> {
        Err(AdapterError::UnsupportedCapability)
    }
}

/// Only loopback-literal HTTPS with authenticated Sunshine API credentials.
/// The local certificate identity is intentionally not checked; the transport
/// boundary is the kernel loopback interface. Proxies, redirects, arbitrary
/// paths and credentials embedded in URLs remain forbidden.
pub struct LocalSunshine {
    client: reqwest::Client,
    base: Url,
    authorization: header::HeaderValue,
    service: LocalServiceController,
    backup: Option<crate::storage::ProtectedState>,
    deadline: Option<crate::engine::ExecutionDeadline>,
    log_snapshot: Option<(tokio::time::Instant, zeroize::Zeroizing<Vec<u8>>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceControlMode {
    #[default]
    Disabled,
    SystemdUser,
    SystemdSystem,
    WindowsService,
}

impl ServiceControlMode {
    pub fn supported(self) -> bool {
        match self {
            Self::Disabled => false,
            Self::SystemdUser | Self::SystemdSystem => cfg!(target_os = "linux"),
            Self::WindowsService => cfg!(target_os = "windows"),
        }
    }
}

const fn platform_service_mode() -> ServiceControlMode {
    if cfg!(target_os = "windows") {
        ServiceControlMode::WindowsService
    } else if cfg!(target_os = "linux") {
        ServiceControlMode::SystemdSystem
    } else {
        ServiceControlMode::Disabled
    }
}

pub(crate) const fn platform_service_control_available() -> bool {
    !matches!(platform_service_mode(), ServiceControlMode::Disabled)
}

struct LocalServiceController {
    mode: ServiceControlMode,
    deadline: Option<crate::engine::ExecutionDeadline>,
}

const SERVICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

async fn service_command_output(
    mut command: tokio::process::Command,
    deadline: Duration,
) -> Result<Output, AdapterError> {
    // A stuck service manager must release the single command execution lane.
    command.kill_on_drop(true);
    tokio::time::timeout(deadline, command.output())
        .await
        .map_err(|_| AdapterError::ApiUnavailable)?
        .map_err(|_| AdapterError::UnsupportedCapability)
}

#[derive(Debug)]
struct LocalCertificateVerifier {
    algorithms: WebPkiSupportedAlgorithms,
}

impl LocalCertificateVerifier {
    fn new() -> Self {
        Self {
            algorithms: aws_lc_rs::default_provider().signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for LocalCertificateVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, certificate, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, certificate, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

impl LocalSunshine {
    pub fn with_backup_directory(mut self, path: &std::path::Path) -> Result<Self, AdapterError> {
        self.backup = Some(
            crate::storage::ProtectedState::open(path)
                .map_err(|_| AdapterError::UnsafeConfiguration)?,
        );
        Ok(self)
    }
    fn require_effect_budget(&self) -> Result<(), AdapterError> {
        if self
            .deadline
            .as_ref()
            .is_some_and(crate::engine::ExecutionDeadline::expired)
        {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(())
    }
    pub fn new(
        endpoint: &str,
        username: &str,
        password: Zeroizing<String>,
    ) -> Result<Self, AdapterError> {
        Self::new_with_service(endpoint, username, password, platform_service_mode())
    }

    pub fn new_with_service(
        endpoint: &str,
        username: &str,
        password: Zeroizing<String>,
        service_mode: ServiceControlMode,
    ) -> Result<Self, AdapterError> {
        if service_mode != ServiceControlMode::Disabled && !service_mode.supported() {
            return Err(AdapterError::InvalidLocalEndpoint);
        }
        let base = Url::parse(endpoint).map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        validate_local_endpoint(&base)?;
        if username.is_empty()
            || username.len() > 256
            || username.contains(':')
            || username.chars().any(char::is_control)
            || password.is_empty()
            || password.len() > 4096
        {
            return Err(AdapterError::InvalidLocalEndpoint);
        }
        let tls = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(LocalCertificateVerifier::new()))
            .with_no_client_auth();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .no_proxy()
            .https_only(true)
            .http2_max_header_list_size(16 * 1024)
            .user_agent(format!("xscc/{}", env!("CARGO_PKG_VERSION")))
            .tls_backend_preconfigured(tls)
            .build()
            .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        let raw = Zeroizing::new(format!("{username}:{}", password.as_str()));
        let encoded = Zeroizing::new(format!("Basic {}", STANDARD.encode(raw.as_bytes())));
        let mut authorization = header::HeaderValue::from_str(&encoded)
            .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        authorization.set_sensitive(true);
        Ok(Self {
            client,
            base,
            authorization,
            service: LocalServiceController {
                mode: service_mode,
                deadline: None,
            },
            backup: None,
            deadline: None,
            log_snapshot: None,
        })
    }

    // Windows backup state contains a Send but non-Sync SQLite connection. Keep
    // the executor's exclusive borrow across awaits instead of requiring Sync.
    async fn request_bytes(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, AdapterError> {
        if !matches!(method, Method::GET | Method::HEAD) {
            self.require_effect_budget()?;
        }
        let mut request = Request::new(
            method,
            self.base
                .join(path)
                .map_err(|_| AdapterError::InvalidLocalEndpoint)?,
        );
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, self.authorization.clone());
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        if let Some(body) = body {
            *request.body_mut() = Some(body.into());
        }
        // /api/pin waits for Moonlight's cryptographic handshake, not just PIN acceptance.
        // Keep this inside the executor's 90-second budget without repeating the POST.
        if request.method() == Method::POST && path == "/api/pin" {
            *request.timeout_mut() = Some(Duration::from_secs(75));
        }
        // Non-browser local client: no Origin/Referer. The pinned release explicitly supports this.
        let (status, bytes) = {
            let mut response = self
                .client
                .execute(request)
                .await
                .map_err(|_| AdapterError::ApiUnavailable)?;
            let header_bytes = response
                .headers()
                .iter()
                .try_fold(0usize, |total, (name, value)| {
                    total.checked_add(name.as_str().len() + value.as_bytes().len() + 4)
                })
                .ok_or(AdapterError::ApiUnavailable)?;
            if header_bytes > 16 * 1024
                || response
                    .content_length()
                    .is_some_and(|length| length > MAX_CONFIG_BYTES as u64)
            {
                return Err(AdapterError::ApiUnavailable);
            }
            let status = response.status();
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| AdapterError::ApiUnavailable)?
            {
                if body
                    .len()
                    .checked_add(chunk.len())
                    .is_none_or(|length| length > MAX_CONFIG_BYTES)
                {
                    return Err(AdapterError::ApiUnavailable);
                }
                body.extend_from_slice(&chunk);
            }
            (status, body)
        };
        if matches!(status.as_u16(), 401 | 403) {
            return Err(AdapterError::CredentialsRejected);
        }
        if status.as_u16() == 426 {
            return Err(AdapterError::UnsupportedVersion {
                detected: None,
                origin: VersionSource::HttpUpgradeRequired,
            });
        }
        if !status.is_success() {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(bytes)
    }

    async fn request(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Value, AdapterError> {
        let bytes = self.request_bytes(method, path, body).await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
        if value.get("status") != Some(&Value::Bool(true)) {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(value)
    }
}

fn sha256_domain(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn applications_from_bytes(bytes: &[u8]) -> Result<ApplicationsSnapshot, AdapterError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
    let apps = value
        .get("apps")
        .and_then(Value::as_array)
        .ok_or(AdapterError::UnsafeConfiguration)?;
    if apps.len() > 256 {
        return Err(AdapterError::UnsafeConfiguration);
    }
    let mut applications = Vec::with_capacity(apps.len());
    let mut canonical = Vec::with_capacity(apps.len());
    let mut references = std::collections::BTreeSet::new();
    for value in apps {
        // Sunshine's sparse apps.json uses true for these flags and empty strings
        // for omitted preparation steps. Normalize its defaults at the API boundary.
        let mut value = value.clone();
        let app = value
            .as_object_mut()
            .ok_or(AdapterError::UnsafeConfiguration)?;
        for key in ["auto-detach", "wait-all"] {
            app.entry(key).or_insert(Value::Bool(true));
        }
        if let Some(commands) = app.get_mut("prep-cmd").and_then(Value::as_array_mut) {
            for command in commands {
                let command = command
                    .as_object_mut()
                    .ok_or(AdapterError::UnsafeConfiguration)?;
                for key in ["do", "undo"] {
                    command
                        .entry(key)
                        .or_insert_with(|| Value::String(String::new()));
                }
            }
        }
        let specification: ApplicationSpec =
            serde_json::from_value(value).map_err(|_| AdapterError::UnsafeConfiguration)?;
        specification
            .validate(true)
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
        let bytes =
            serde_json::to_vec(&specification).map_err(|_| AdapterError::UnsafeConfiguration)?;
        let reference = xscs_protocol::application_reference(&specification)
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
        if !references.insert(reference.fingerprint.clone()) {
            return Err(AdapterError::ResourceConflict);
        }
        canonical.push(bytes);
        applications.push(ApplicationView {
            reference,
            specification,
        });
    }
    let bytes = serde_json::to_vec(&canonical).map_err(|_| AdapterError::UnsafeConfiguration)?;
    Ok(ApplicationsSnapshot {
        revision: sha256_domain(b"sunshine-applications-v1\0", &bytes),
        applications,
    })
}

fn paired_clients_from_value(value: Value) -> Result<PairedClientsSnapshot, AdapterError> {
    let values = value
        .get("named_certs")
        .and_then(Value::as_array)
        .ok_or(AdapterError::UnsafeConfiguration)?;
    if values.len() > 512 {
        return Err(AdapterError::UnsafeConfiguration);
    }
    let mut clients = Vec::with_capacity(values.len());
    for value in values {
        let uuid = value
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or(AdapterError::UnsafeConfiguration)?
            .to_ascii_lowercase();
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or(AdapterError::UnsafeConfiguration)?
            .to_owned();
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or(AdapterError::UnsafeConfiguration)?;
        if uuid::Uuid::parse_str(&uuid).is_err()
            || name.is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
        {
            return Err(AdapterError::UnsafeConfiguration);
        }
        clients.push(PairedClient {
            uuid,
            name,
            enabled,
        });
    }
    clients.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    let bytes = serde_json::to_vec(&clients).map_err(|_| AdapterError::UnsafeConfiguration)?;
    Ok(PairedClientsSnapshot {
        revision: sha256_domain(b"sunshine-paired-clients-v1\0", &bytes),
        clients,
    })
}

fn log_page(bytes: &[u8], cursor: Option<&LogCursor>, limit: u32) -> Result<LogPage, AdapterError> {
    let source = std::str::from_utf8(bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
    let mut redacted = String::with_capacity(source.len());
    let mut changed = false;
    for segment in source.split_inclusive('\n') {
        let lower = segment.to_ascii_lowercase();
        if ["password", "authorization:", "bearer ", "token=", "pin="]
            .iter()
            .any(|needle| lower.contains(needle))
        {
            redacted.push_str("[redacted]\n");
            changed = true;
        } else {
            redacted.push_str(segment);
        }
    }
    let revision = sha256_domain(b"sunshine-log-v1\0", redacted.as_bytes());
    let mut end = cursor.map_or(redacted.len(), |cursor| {
        if cursor.revision != revision {
            return usize::MAX;
        }
        usize::try_from(cursor.before_offset).unwrap_or(usize::MAX)
    });
    if end > redacted.len() {
        return Err(AdapterError::ResourceConflict);
    }
    while end > 0 && !redacted.is_char_boundary(end) {
        end -= 1
    }
    let mut start = end.saturating_sub(limit as usize);
    while start < end && !redacted.is_char_boundary(start) {
        start += 1
    }
    let previous = (start > 0).then(|| LogCursor {
        revision: revision.clone(),
        before_offset: start as u64,
    });
    Ok(LogPage {
        revision,
        text: redacted[start..end].to_owned(),
        start_offset: start as u64,
        end_offset: end as u64,
        total_bytes: redacted.len() as u64,
        previous,
        redacted: changed,
    })
}

impl LocalServiceController {
    async fn status(&self) -> Result<ServiceState, AdapterError> {
        match self.mode {
            ServiceControlMode::Disabled => Ok(ServiceState::Unavailable),
            ServiceControlMode::SystemdUser | ServiceControlMode::SystemdSystem => {
                let mut command = tokio::process::Command::new("systemctl");
                if self.mode == ServiceControlMode::SystemdUser {
                    command.arg("--user");
                }
                command.args([
                    "show",
                    "--property=ActiveState",
                    "--value",
                    "sunshine.service",
                ]);
                let output = service_command_output(command, SERVICE_COMMAND_TIMEOUT).await?;
                if !output.status.success() {
                    return Ok(ServiceState::Unknown);
                }
                Ok(match String::from_utf8_lossy(&output.stdout).trim() {
                    "active" => ServiceState::Running,
                    "inactive" | "failed" => ServiceState::Stopped,
                    "activating" => ServiceState::Starting,
                    "deactivating" => ServiceState::Stopping,
                    _ => ServiceState::Unknown,
                })
            }
            ServiceControlMode::WindowsService => {
                let mut command = tokio::process::Command::new("sc.exe");
                command.args(["query", "SunshineService"]);
                let output = service_command_output(command, SERVICE_COMMAND_TIMEOUT).await?;
                if !output.status.success() {
                    return Ok(ServiceState::Unknown);
                }
                let text = String::from_utf8_lossy(&output.stdout);
                let state = text
                    .lines()
                    .find(|line| line.contains("STATE"))
                    .and_then(|line| line.split(':').nth(1))
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<u32>().ok());
                Ok(match state {
                    Some(4) => ServiceState::Running,
                    Some(1) => ServiceState::Stopped,
                    Some(2) => ServiceState::Starting,
                    Some(3) => ServiceState::Stopping,
                    _ => ServiceState::Unknown,
                })
            }
        }
    }
    async fn control(&self, action: ServiceAction) -> Result<ServiceState, AdapterError> {
        let started = crate::elapsed_clock::milliseconds().ok_or(AdapterError::ApiUnavailable)?;
        tokio::time::timeout(Duration::from_secs(45), self.control_inner(action, started))
            .await
            .map_err(|_| AdapterError::ApiUnavailable)?
    }
    fn control_budget(&self, started: u64) -> Result<(), AdapterError> {
        let now = crate::elapsed_clock::milliseconds().ok_or(AdapterError::ApiUnavailable)?;
        if now < started
            || now - started >= 45_000
            || self
                .deadline
                .as_ref()
                .is_some_and(crate::engine::ExecutionDeadline::expired)
        {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(())
    }
    async fn control_inner(
        &self,
        action: ServiceAction,
        started: u64,
    ) -> Result<ServiceState, AdapterError> {
        if self.mode == ServiceControlMode::Disabled {
            return Err(AdapterError::UnsupportedCapability);
        }
        let windows_state = if self.mode == ServiceControlMode::WindowsService {
            Some(self.status().await?)
        } else {
            None
        };
        if let Some(state) = windows_state
            && matches!(
                (action, state),
                (ServiceAction::Start, ServiceState::Running)
                    | (ServiceAction::Stop, ServiceState::Stopped)
            )
        {
            return Ok(state);
        }
        let mut command = match self.mode {
            ServiceControlMode::SystemdUser | ServiceControlMode::SystemdSystem => {
                let mut command = tokio::process::Command::new("systemctl");
                if self.mode == ServiceControlMode::SystemdUser {
                    command.arg("--user");
                }
                command
                    .arg(match action {
                        ServiceAction::Start => "start",
                        ServiceAction::Stop => "stop",
                        ServiceAction::Restart => "restart",
                    })
                    .arg("sunshine.service");
                command
            }
            ServiceControlMode::WindowsService => {
                let mut command = tokio::process::Command::new("sc.exe");
                command.args([
                    match action {
                        ServiceAction::Start => "start",
                        ServiceAction::Stop => "stop",
                        ServiceAction::Restart if windows_state == Some(ServiceState::Stopped) => {
                            "start"
                        }
                        ServiceAction::Restart => "stop",
                    },
                    "SunshineService",
                ]);
                command
            }
            ServiceControlMode::Disabled => unreachable!(),
        };
        self.control_budget(started)?;
        let status = service_command_output(command, SERVICE_COMMAND_TIMEOUT).await?;
        if !status.status.success() {
            return Err(AdapterError::ApiUnavailable);
        }
        if self.mode == ServiceControlMode::WindowsService
            && action == ServiceAction::Restart
            && windows_state != Some(ServiceState::Stopped)
        {
            for _ in 0..15 {
                if self.status().await? == ServiceState::Stopped {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if self.status().await? != ServiceState::Stopped {
                return Err(AdapterError::ApiUnavailable);
            }
            command = tokio::process::Command::new("sc.exe");
            command.args(["start", "SunshineService"]);
            self.control_budget(started)?;
            if !service_command_output(command, SERVICE_COMMAND_TIMEOUT)
                .await?
                .status
                .success()
            {
                return Err(AdapterError::ApiUnavailable);
            }
        }
        let expected = match action {
            ServiceAction::Stop => ServiceState::Stopped,
            _ => ServiceState::Running,
        };
        for _ in 0..15 {
            let observed = self.status().await?;
            if observed == expected {
                return Ok(observed);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let observed = self.status().await?;
        if observed == expected {
            Ok(observed)
        } else {
            Err(AdapterError::ApiUnavailable)
        }
    }
}

pub fn validate_local_endpoint(url: &Url) -> Result<(), AdapterError> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !url.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
    {
        return Err(AdapterError::InvalidLocalEndpoint);
    }
    Ok(())
}

#[async_trait]
impl Sunshine for LocalSunshine {
    fn set_execution_deadline(&mut self, deadline: Option<crate::engine::ExecutionDeadline>) {
        self.service.deadline = deadline.clone();
        self.deadline = deadline;
    }
    async fn save_overwrite(
        &mut self,
        before: &Configuration,
        configuration: &Configuration,
    ) -> Result<(), AdapterError> {
        if let Some(backup) = &self.backup {
            let bytes = serde_json::to_vec(&json!({
                "before_revision": before.revision(),
                "target_revision": configuration.revision(),
                "configuration": before.fields
            }))
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
            backup
                .put("latest.json", &bytes)
                .map_err(|_| AdapterError::UnsafeConfiguration)?;
        }
        self.require_effect_budget()?;
        self.save(configuration).await
    }
    async fn save_guarded(
        &mut self,
        expected: &str,
        configuration: &Configuration,
    ) -> Result<(), AdapterError> {
        let before = self.read().await?;
        if before.revision() != expected {
            return Err(AdapterError::ResourceConflict);
        }
        if let Some(backup) = &self.backup {
            let bytes = serde_json::to_vec(&json!({"before_revision":expected,"target_revision":configuration.revision(),"configuration":before.fields}))
                .map_err(|_|AdapterError::UnsafeConfiguration)?;
            backup
                .put("latest.json", &bytes)
                .map_err(|_| AdapterError::UnsafeConfiguration)?;
        }
        // Do not roll back over a local editor. Sunshine has no native atomic CAS endpoint.
        if self.read().await?.revision() != expected {
            return Err(AdapterError::ResourceConflict);
        }
        self.require_effect_budget()?;
        self.save(configuration).await
    }
    async fn read(&mut self) -> Result<Configuration, AdapterError> {
        Configuration::from_response(self.request(Method::GET, "/api/config", None).await?)
    }

    async fn save(&mut self, configuration: &Configuration) -> Result<(), AdapterError> {
        self.request(Method::POST, "/api/config", Some(configuration.encoded()?))
            .await?;
        Ok(())
    }

    async fn restart(&mut self) -> Result<(), AdapterError> {
        // The pinned endpoint invokes platf::restart without a JSON response contract.
        // A connection error is ambiguous; the engine verifies a new process, never retries.
        self.require_effect_budget()?;
        let response = self
            .client
            .post(
                self.base
                    .join("/api/restart")
                    .map_err(|_| AdapterError::InvalidLocalEndpoint)?,
            )
            .header(header::AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(|_| AdapterError::ApiUnavailable)?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(AdapterError::CredentialsRejected);
        }
        if !response.status().is_success() {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(())
    }

    async fn process_generation(&mut self) -> Option<String> {
        let port = self.base.port_or_known_default()?;
        tokio::task::spawn_blocking(move || crate::process_identity::sunshine_generation(port))
            .await
            .ok()
            .flatten()
    }

    async fn applications(&mut self) -> Result<ApplicationsSnapshot, AdapterError> {
        applications_from_bytes(&self.request_bytes(Method::GET, "/api/apps", None).await?)
    }

    async fn save_application(
        &mut self,
        expected: &str,
        target: Option<&ApplicationRef>,
        application: &ApplicationSpec,
    ) -> Result<ApplicationsSnapshot, AdapterError> {
        let current = self.applications().await?;
        if current.revision != expected {
            return Err(AdapterError::ResourceConflict);
        }
        let index = match target {
            None => -1,
            Some(target) => {
                let matches = current
                    .applications
                    .iter()
                    .enumerate()
                    .filter(|(_, app)| app.reference == *target)
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(AdapterError::ResourceConflict);
                }
                i64::try_from(matches[0]).map_err(|_| AdapterError::UnsafeConfiguration)?
            }
        };
        let mut value =
            serde_json::to_value(application).map_err(|_| AdapterError::UnsafeConfiguration)?;
        value
            .as_object_mut()
            .ok_or(AdapterError::UnsafeConfiguration)?
            .insert("index".into(), Value::from(index));
        self.request(
            Method::POST,
            "/api/apps",
            Some(serde_json::to_vec(&value).map_err(|_| AdapterError::UnsafeConfiguration)?),
        )
        .await?;
        self.applications().await
    }

    async fn delete_application(
        &mut self,
        expected: &str,
        target: &ApplicationRef,
    ) -> Result<ApplicationsSnapshot, AdapterError> {
        let current = self.applications().await?;
        if current.revision != expected {
            return Err(AdapterError::ResourceConflict);
        }
        let matches = current
            .applications
            .iter()
            .enumerate()
            .filter(|(_, app)| app.reference == *target)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(AdapterError::ResourceConflict);
        }
        self.request(Method::DELETE, &format!("/api/apps/{}", matches[0]), None)
            .await?;
        self.applications().await
    }

    async fn close_application(&mut self) -> Result<(), AdapterError> {
        self.request(Method::POST, "/api/apps/close", None).await?;
        Ok(())
    }

    async fn upload_cover(&mut self, key: &str, png: &[u8]) -> Result<String, AdapterError> {
        if png.len() > 30 * 1024 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(AdapterError::UnsafeConfiguration);
        }
        let value = self
            .request(
                Method::POST,
                "/api/covers/upload",
                Some(
                    serde_json::to_vec(&json!({"key":key,"data":STANDARD.encode(png)}))
                        .map_err(|_| AdapterError::UnsafeConfiguration)?,
                ),
            )
            .await?;
        value
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| {
                !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
            })
            .map(str::to_owned)
            .ok_or(AdapterError::UnsafeConfiguration)
    }

    async fn submit_pairing_pin(
        &mut self,
        pairing_id: &str,
        pin: &str,
        name: &str,
    ) -> Result<(), AdapterError> {
        let bytes = self
            .request_bytes(
                Method::POST,
                "/api/pin",
                Some(
                    serde_json::to_vec(&json!({"pairing_id":pairing_id,"pin":pin,"name":name}))
                        .map_err(|_| AdapterError::UnsafeConfiguration)?,
                ),
            )
            .await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
        match value.get("status").and_then(Value::as_bool) {
            Some(true) => Ok(()),
            Some(false) => Err(AdapterError::PairingRejected),
            None => Err(AdapterError::UnsafeConfiguration),
        }
    }

    async fn pending_pairings(&mut self) -> Result<Vec<PendingPairing>, AdapterError> {
        let bytes = self.request_bytes(Method::GET, "/api/pin", None).await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
        let pairings: Vec<PendingPairing> = serde_json::from_value(
            value
                .get("pairings")
                .cloned()
                .ok_or(AdapterError::UnsafeConfiguration)?,
        )
        .map_err(|_| AdapterError::UnsafeConfiguration)?;
        xscs_protocol::validate_pending_pairings(&pairings)
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
        Ok(pairings)
    }

    async fn paired_clients(&mut self) -> Result<PairedClientsSnapshot, AdapterError> {
        paired_clients_from_value(self.request(Method::GET, "/api/clients/list", None).await?)
    }

    async fn set_paired_client_enabled(
        &mut self,
        uuid: &str,
        enabled: bool,
    ) -> Result<PairedClientsSnapshot, AdapterError> {
        self.request(
            Method::POST,
            "/api/clients/update",
            Some(
                serde_json::to_vec(&json!({"uuid":uuid,"enabled":enabled}))
                    .map_err(|_| AdapterError::UnsafeConfiguration)?,
            ),
        )
        .await?;
        self.paired_clients().await
    }

    async fn unpair_client(&mut self, uuid: &str) -> Result<PairedClientsSnapshot, AdapterError> {
        self.request(
            Method::POST,
            "/api/clients/unpair",
            Some(
                serde_json::to_vec(&json!({"uuid":uuid}))
                    .map_err(|_| AdapterError::UnsafeConfiguration)?,
            ),
        )
        .await?;
        self.paired_clients().await
    }

    async fn unpair_all_clients(&mut self) -> Result<PairedClientsSnapshot, AdapterError> {
        self.request(Method::POST, "/api/clients/unpair-all", None)
            .await?;
        self.paired_clients().await
    }

    async fn logs(
        &mut self,
        cursor: Option<&LogCursor>,
        limit: u32,
    ) -> Result<LogPage, AdapterError> {
        if let Some(cursor) = cursor {
            let (created, bytes) = self
                .log_snapshot
                .as_ref()
                .filter(|(created, _)| created.elapsed() < Duration::from_secs(120))
                .ok_or(AdapterError::ResourceConflict)?;
            let _ = created;
            return log_page(bytes, Some(cursor), limit);
        }
        // Upstream serves a whole file. Stream it with bounded memory, bytes and elapsed time.
        // Paging covers the retained tail window; an oversized upstream stream fails explicitly.
        const WINDOW: usize = 1024 * 1024;
        const MAX_STREAM: usize = 64 * 1024 * 1024;
        let mut response = self
            .client
            .get(
                self.base
                    .join("/api/logs")
                    .map_err(|_| AdapterError::InvalidLocalEndpoint)?,
            )
            .header(header::AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(|_| AdapterError::ApiUnavailable)?;
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
            .sum::<usize>();
        if headers > 16 * 1024 {
            return Err(AdapterError::ApiUnavailable);
        }
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(AdapterError::CredentialsRejected);
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_STREAM as u64)
        {
            return Err(AdapterError::ApiUnavailable);
        }
        let mut tail = Vec::with_capacity(WINDOW);
        let mut total = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AdapterError::ApiUnavailable)?
        {
            total = total
                .checked_add(chunk.len())
                .ok_or(AdapterError::ApiUnavailable)?;
            if total > MAX_STREAM {
                return Err(AdapterError::ApiUnavailable);
            }
            if chunk.len() >= WINDOW {
                tail.clear();
                tail.extend_from_slice(&chunk[chunk.len() - WINDOW..]);
            } else {
                let excess = (tail.len() + chunk.len()).saturating_sub(WINDOW);
                if excess > 0 {
                    tail.drain(..excess);
                }
                tail.extend_from_slice(&chunk);
            }
        }
        if total > tail.len() {
            // Discard the partial first line, including any incomplete UTF-8 or secret prefix.
            let line = tail
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(tail.len(), |at| at + 1);
            tail.drain(..line);
        }
        let page = log_page(&tail, cursor, limit)?;
        self.log_snapshot = Some((tokio::time::Instant::now(), zeroize::Zeroizing::new(tail)));
        Ok(page)
    }

    async fn virtual_input_status(&mut self) -> Result<VirtualInputStatus, AdapterError> {
        fn driver(value: &Value) -> Result<DriverStatus, AdapterError> {
            Ok(DriverStatus {
                installed: value
                    .get("installed")
                    .and_then(Value::as_bool)
                    .ok_or(AdapterError::UnsafeConfiguration)?,
                version: value
                    .get("version")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                required_version: value
                    .get("minimum_version")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                error: value
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.chars().take(256).collect()),
            })
        }
        let bytes = self
            .request_bytes(Method::GET, "/api/virtual-input/status", None)
            .await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
        Ok(VirtualInputStatus {
            virtualhid: driver(
                value
                    .get("virtualhid")
                    .ok_or(AdapterError::UnsafeConfiguration)?,
            )?,
            vigembus: driver(
                value
                    .get("vigembus")
                    .ok_or(AdapterError::UnsafeConfiguration)?,
            )?,
        })
    }

    async fn maintenance(&mut self, action: MaintenanceAction) -> Result<(), AdapterError> {
        let path = match action {
            MaintenanceAction::ResetDisplayPersistence => "/api/reset-display-device-persistence",
            MaintenanceAction::ResetPortalToken => "/api/reset-portal-token",
        };
        self.request(Method::POST, path, None).await?;
        Ok(())
    }

    async fn service_status(&mut self) -> Result<ServiceState, AdapterError> {
        self.service.status().await
    }
    async fn control_service(
        &mut self,
        action: ServiceAction,
    ) -> Result<ServiceState, AdapterError> {
        self.require_effect_budget()?;
        self.service.control(action).await
    }
}

#[cfg(test)]
mod application_mapping_tests {
    use super::*;

    fn snapshot(application: Value) -> Result<ApplicationsSnapshot, AdapterError> {
        applications_from_bytes(&serde_json::to_vec(&json!({"apps": [application]})).unwrap())
    }

    #[test]
    fn sparse_application_uses_sunshine_launch_defaults() {
        let sparse = snapshot(json!({"name": "Game launcher", "cmd": "launcher"})).unwrap();
        let explicit = snapshot(json!({
            "name": "Game launcher", "cmd": "launcher",
            "auto-detach": true, "wait-all": true
        }))
        .unwrap();
        assert!(sparse.applications[0].specification.auto_detach);
        assert!(sparse.applications[0].specification.wait_all);
        assert_eq!(
            sparse, explicit,
            "equivalent upstream defaults have stable references"
        );
    }

    #[test]
    fn one_way_preparation_commands_fill_only_the_missing_step() {
        let sparse = snapshot(json!({"name": "Desktop", "prep-cmd": [
            {"do": "prepare"}, {"undo": "restore"}
        ]}))
        .unwrap();
        let explicit = snapshot(json!({"name": "Desktop", "prep-cmd": [
            {"do": "prepare", "undo": ""}, {"do": "", "undo": "restore"}
        ]}))
        .unwrap();
        assert_eq!(sparse, explicit);
        let commands = &sparse.applications[0].specification.prep_cmd;
        assert_eq!(commands[0].execute, "prepare");
        assert!(commands[0].undo.is_empty());
        assert!(commands[1].execute.is_empty());
        assert_eq!(commands[1].undo, "restore");
    }

    #[test]
    fn explicit_false_and_empty_application_values_are_preserved() {
        let explicit = snapshot(json!({"name": "Desktop", "cmd": "", "output": "",
            "auto-detach": false, "wait-all": false,
            "prep-cmd": [{"do": "prepare", "undo": "", "elevated": false}]
        }))
        .unwrap();
        let application = &explicit.applications[0].specification;
        assert!(!application.auto_detach);
        assert!(!application.wait_all);
        assert!(application.cmd.is_empty());
        assert!(application.output.is_empty());
        assert!(application.prep_cmd[0].undo.is_empty());
        assert!(!application.prep_cmd[0].elevated);
        let partial = snapshot(json!({"name": "Desktop", "auto-detach": false})).unwrap();
        assert!(!partial.applications[0].specification.auto_detach);
        assert!(partial.applications[0].specification.wait_all);
    }

    #[test]
    fn normalization_keeps_existing_type_and_content_validation() {
        for invalid in [
            json!({"name": "Desktop", "auto-detach": "true"}),
            json!({"name": "Desktop", "auto-detach": null}),
            json!({"name": "Desktop", "wait-all": 1}),
            json!({"name": "Desktop", "prep-cmd": null}),
            json!({"name": "Desktop", "prep-cmd": ["prepare"]}),
            json!({"name": "Desktop", "prep-cmd": [{"do": null, "undo": "restore"}]}),
            json!({"name": "Desktop", "prep-cmd": [{"do": "prepare", "undo": false}]}),
            json!({"name": "Desktop", "prep-cmd": [{}]}),
            json!({"name": "Desktop", "prep-cmd": [{"do": "", "undo": ""}]}),
            json!({"name": "Desktop", "unknown-setting": true}),
        ] {
            assert_eq!(snapshot(invalid), Err(AdapterError::UnsafeConfiguration));
        }
    }
}

#[cfg(test)]
mod local_tls_tests {
    use super::*;

    #[test]
    fn loopback_verifier_does_not_authenticate_certificate_identity() {
        let verifier = LocalCertificateVerifier::new();
        let name = ServerName::try_from("127.0.0.1").unwrap();
        assert!(
            verifier
                .verify_server_cert(
                    &CertificateDer::from(vec![1, 2, 3]),
                    &[],
                    &name,
                    &[],
                    UnixTime::since_unix_epoch(Duration::from_secs(0)),
                )
                .is_ok()
        );
        assert!(
            verifier
                .verify_server_cert(
                    &CertificateDer::from(vec![3, 2, 1]),
                    &[],
                    &name,
                    &[],
                    UnixTime::since_unix_epoch(Duration::from_secs(0)),
                )
                .is_ok()
        );
    }
}

#[cfg(all(test, unix))]
mod service_command_tests {
    use super::*;

    #[tokio::test]
    async fn hung_service_manager_command_has_a_deadline() {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("5");
        let started = tokio::time::Instant::now();
        let result = service_command_output(command, Duration::from_millis(25)).await;
        assert_eq!(result.unwrap_err(), AdapterError::ApiUnavailable);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
