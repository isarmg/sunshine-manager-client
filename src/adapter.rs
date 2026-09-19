use std::{
    collections::BTreeMap,
    error::Error as StdError,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Method, Request, header, redirect::Policy};
use rustls_platform_verifier::ConfigVerifierExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sunshine_client_protocol::{
    ApplicationRef, ApplicationSpec, ApplicationView, ApplicationsSnapshot, ConfigSnapshot,
    DriverStatus, Effectiveness, LogCursor, LogPage, MaintenanceAction, PairedClient,
    PairedClientsSnapshot, ServiceAction, ServiceState, VirtualInputStatus,
};
use tokio_rustls::rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    client::{
        Resumption,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{
        WebPkiSupportedAlgorithms, aws_lc_rs, verify_tls12_signature, verify_tls13_signature,
    },
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};
use url::Url;
use zeroize::Zeroizing;

pub const MAX_CONFIG_BYTES: usize = 512 * 1024;
const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;

/// Never print upstream errors/bodies: they may include local settings or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("Sunshine certificate is not trusted by the selected system trust store")]
    CertificateUntrusted,
    #[error("Sunshine presented a certificate different from the configured pin")]
    CertificateMismatch,
    #[error("Sunshine rejected the configured local credentials")]
    CredentialsRejected,
    #[error("Sunshine HTTPS API is unavailable")]
    ApiUnavailable,
    #[error("unsupported Sunshine version")]
    UnsupportedVersion,
    #[error("unsafe or malformed Sunshine configuration")]
    UnsafeConfiguration,
    #[error("invalid local HTTPS configuration")]
    InvalidLocalEndpoint,
    #[error("Sunshine resource changed before the operation could be applied")]
    ResourceConflict,
    #[error("the locally selected adapter does not support this capability")]
    UnsupportedCapability,
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
        if !sunshine_client_protocol::is_supported_sunshine_version(&sunshine_version) {
            return Err(AdapterError::UnsupportedVersion);
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
                || (sunshine_client_protocol::config::contains_field(&key) && value.len() > 512)
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
        format!("{:x}", digest.finalize())
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
                    sunshine_client_protocol::config::normalized_snapshot_value(key, value)
                        .filter(|value| {
                            sunshine_client_protocol::config::validate_snapshot_field(key, value)
                                .is_ok()
                        })
                        .map(|value| (key.clone(), value))
                })
                .collect(),
            effectiveness,
        }
    }

    pub(crate) fn merge(
        &self,
        set: &BTreeMap<String, sunshine_client_protocol::config::FieldValue>,
        remove: &std::collections::BTreeSet<String>,
    ) -> Result<Self, AdapterError> {
        let mut merged = self.clone();
        for (key, value) in set {
            sunshine_client_protocol::config::validate_field(key, value)
                .map_err(|_| AdapterError::UnsafeConfiguration)?;
            if remove.contains(key) {
                return Err(AdapterError::UnsafeConfiguration);
            }
            merged.fields.insert(key.clone(), value.sunshine_text());
        }
        for key in remove {
            if !sunshine_client_protocol::config::contains_field(key) {
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
    async fn read(&mut self) -> Result<Configuration, AdapterError>;
    async fn save(&mut self, configuration: &Configuration) -> Result<(), AdapterError>;
    async fn restart(&mut self) -> Result<(), AdapterError>;
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

/// Only loopback-literal HTTPS; caller supplies a locally provisioned trust anchor.
/// No proxy, redirect, arbitrary paths, insecure TLS option, or network credentials in DTOs.
pub struct LocalSunshine {
    client: LocalClient,
    base: Url,
    authorization: header::HeaderValue,
    service: LocalServiceController,
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
}

enum LocalClient {
    System(reqwest::Client),
    Pinned {
        client: reqwest::Client,
        mismatch_seen: Arc<AtomicBool>,
    },
}

#[derive(Debug)]
struct ExactCertificateVerifier {
    certificate: CertificateDer<'static>,
    algorithms: WebPkiSupportedAlgorithms,
    mismatch_seen: Arc<AtomicBool>,
}

impl ExactCertificateVerifier {
    fn from_pem(pem: &[u8]) -> Result<Self, AdapterError> {
        if pem.is_empty() || pem.len() > MAX_CERTIFICATE_BYTES {
            return Err(AdapterError::InvalidLocalEndpoint);
        }
        let text = std::str::from_utf8(pem).map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        if text.matches("-----BEGIN CERTIFICATE-----").count() != 1
            || text.matches("-----END CERTIFICATE-----").count() != 1
            || text.contains("PRIVATE KEY")
        {
            return Err(AdapterError::InvalidLocalEndpoint);
        }
        let certificates = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        let [certificate] = certificates.as_slice() else {
            return Err(AdapterError::InvalidLocalEndpoint);
        };
        Ok(Self {
            certificate: certificate.clone().into_owned(),
            algorithms: aws_lc_rs::default_provider().signature_verification_algorithms,
            mismatch_seen: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl ServerCertVerifier for ExactCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if end_entity.as_ref() != self.certificate.as_ref() {
            self.mismatch_seen.store(true, Ordering::Release);
            return Err(TlsError::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ));
        }
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
    pub fn new(
        endpoint: &str,
        username: &str,
        password: Zeroizing<String>,
        sunshine_certificate_pem: &[u8],
    ) -> Result<Self, AdapterError> {
        Self::new_with_service(
            endpoint,
            username,
            password,
            sunshine_certificate_pem,
            platform_service_mode(),
        )
    }

    pub fn new_with_service(
        endpoint: &str,
        username: &str,
        password: Zeroizing<String>,
        sunshine_certificate_pem: &[u8],
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
        let client = if sunshine_certificate_pem.is_empty() {
            let tls = ClientConfig::with_platform_verifier()
                .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
            LocalClient::System(
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(15))
                    .connect_timeout(Duration::from_secs(10))
                    .redirect(Policy::none())
                    .no_proxy()
                    .https_only(true)
                    .http2_max_header_list_size(16 * 1024)
                    .user_agent(format!("sunshine-client/{}", env!("CARGO_PKG_VERSION")))
                    .tls_backend_preconfigured(tls)
                    .build()
                    .map_err(|_| AdapterError::InvalidLocalEndpoint)?,
            )
        } else {
            let verifier = Arc::new(ExactCertificateVerifier::from_pem(
                sunshine_certificate_pem,
            )?);
            let mismatch_seen = Arc::clone(&verifier.mismatch_seen);
            let mut tls = ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth();
            // Re-check the exact leaf on every connection; a resumed session may omit it.
            tls.resumption = Resumption::disabled();
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(10))
                .redirect(Policy::none())
                .no_proxy()
                .https_only(true)
                .http2_max_header_list_size(16 * 1024)
                .user_agent(format!("sunshine-client/{}", env!("CARGO_PKG_VERSION")))
                .tls_backend_preconfigured(tls)
                .build()
                .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
            LocalClient::Pinned {
                client,
                mismatch_seen,
            }
        };
        let raw = Zeroizing::new(format!("{username}:{}", password.as_str()));
        let encoded = Zeroizing::new(format!("Basic {}", STANDARD.encode(raw.as_bytes())));
        let mut authorization = header::HeaderValue::from_str(&encoded)
            .map_err(|_| AdapterError::InvalidLocalEndpoint)?;
        authorization.set_sensitive(true);
        Ok(Self {
            client,
            base,
            authorization,
            service: LocalServiceController { mode: service_mode },
        })
    }

    async fn request_bytes(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, AdapterError> {
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
        // Non-browser local client: no Origin/Referer. The pinned release explicitly supports this.
        let (status, bytes) = match &self.client {
            LocalClient::System(client) => {
                let mut response = client.execute(request).await.map_err(|error| {
                    if error_chain_has_certificate_error(&error) {
                        AdapterError::CertificateUntrusted
                    } else {
                        AdapterError::ApiUnavailable
                    }
                })?;
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
            }
            LocalClient::Pinned {
                client,
                mismatch_seen,
            } => {
                mismatch_seen.store(false, Ordering::Release);
                let mut response = client.execute(request).await.map_err(|_| {
                    if mismatch_seen.swap(false, Ordering::AcqRel) {
                        AdapterError::CertificateMismatch
                    } else {
                        AdapterError::ApiUnavailable
                    }
                })?;
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
            }
        };
        if matches!(status.as_u16(), 401 | 403) {
            return Err(AdapterError::CredentialsRejected);
        }
        if status.as_u16() == 426 {
            return Err(AdapterError::UnsupportedVersion);
        }
        if !status.is_success() {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(bytes)
    }

    async fn request(
        &self,
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
    format!("{:x}", digest.finalize())
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
        let specification: ApplicationSpec =
            serde_json::from_value(value.clone()).map_err(|_| AdapterError::UnsafeConfiguration)?;
        specification
            .validate(true)
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
        let bytes =
            serde_json::to_vec(&specification).map_err(|_| AdapterError::UnsafeConfiguration)?;
        let reference = sunshine_client_protocol::application_reference(&specification)
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
                let output = command
                    .args([
                        "show",
                        "--property=ActiveState",
                        "--value",
                        "sunshine.service",
                    ])
                    .output()
                    .await
                    .map_err(|_| AdapterError::UnsupportedCapability)?;
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
                let output = tokio::process::Command::new("sc.exe")
                    .args(["query", "SunshineService"])
                    .output()
                    .await
                    .map_err(|_| AdapterError::UnsupportedCapability)?;
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
        if self.mode == ServiceControlMode::Disabled {
            return Err(AdapterError::UnsupportedCapability);
        }
        let status = match self.mode {
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
                    .arg("sunshine.service")
                    .status()
                    .await
            }
            ServiceControlMode::WindowsService => {
                tokio::process::Command::new("sc.exe")
                    .args([
                        match action {
                            ServiceAction::Start => "start",
                            ServiceAction::Stop => "stop",
                            ServiceAction::Restart => "stop",
                        },
                        "SunshineService",
                    ])
                    .status()
                    .await
            }
            ServiceControlMode::Disabled => unreachable!(),
        }
        .map_err(|_| AdapterError::UnsupportedCapability)?;
        if !status.success() {
            return Err(AdapterError::ApiUnavailable);
        }
        if self.mode == ServiceControlMode::WindowsService && action == ServiceAction::Restart {
            for _ in 0..15 {
                if self.status().await? == ServiceState::Stopped {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if self.status().await? != ServiceState::Stopped {
                return Err(AdapterError::ApiUnavailable);
            }
            if !tokio::process::Command::new("sc.exe")
                .args(["start", "SunshineService"])
                .status()
                .await
                .map_err(|_| AdapterError::UnsupportedCapability)?
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
        self.status().await
    }
}

fn error_chain_has_certificate_error(error: &(dyn StdError + 'static)) -> bool {
    fn contains(error: &(dyn StdError + 'static), depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        if matches!(
            error.downcast_ref::<TlsError>(),
            Some(TlsError::InvalidCertificate(_))
        ) {
            return true;
        }
        // hyper-rustls wraps the TLS failure in nested `io::Error` values whose
        // inner error is not always exposed through `Error::source`.
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && contains(inner, depth + 1)
        {
            return true;
        }
        error
            .source()
            .is_some_and(|source| contains(source, depth + 1))
    }

    contains(error, 0)
}

pub(crate) fn certificate_sha256_fingerprint(pem: &[u8]) -> Result<String, AdapterError> {
    let verifier = ExactCertificateVerifier::from_pem(pem)?;
    let digest = Sha256::digest(verifier.certificate.as_ref());
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
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
    async fn read(&mut self) -> Result<Configuration, AdapterError> {
        Configuration::from_response(self.request(Method::GET, "/api/config", None).await?)
    }

    async fn save(&mut self, configuration: &Configuration) -> Result<(), AdapterError> {
        self.request(Method::POST, "/api/config", Some(configuration.encoded()?))
            .await?;
        Ok(())
    }

    async fn restart(&mut self) -> Result<(), AdapterError> {
        self.request(Method::POST, "/api/restart", None).await?;
        Ok(())
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
        self.request(
            Method::POST,
            "/api/pin",
            Some(
                serde_json::to_vec(&json!({"pairing_id":pairing_id,"pin":pin,"name":name}))
                    .map_err(|_| AdapterError::UnsafeConfiguration)?,
            ),
        )
        .await?;
        Ok(())
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
        log_page(
            &self.request_bytes(Method::GET, "/api/logs", None).await?,
            cursor,
            limit,
        )
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
        self.service.control(action).await
    }
}

#[cfg(test)]
mod certificate_tests {
    use super::*;

    #[test]
    fn pinned_verifier_accepts_only_the_configured_certificate() {
        let verifier = ExactCertificateVerifier {
            certificate: CertificateDer::from(vec![1, 2, 3]),
            algorithms: aws_lc_rs::default_provider().signature_verification_algorithms,
            mismatch_seen: Arc::new(AtomicBool::new(false)),
        };
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
                .is_err()
        );
    }

    #[test]
    fn certificate_input_rejects_private_keys_and_multiple_or_malformed_certificates() {
        for pem in [
            b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n".as_slice(),
            b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".as_slice(),
            b"-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----\n".as_slice(),
        ] {
            assert!(ExactCertificateVerifier::from_pem(pem).is_err());
        }
    }
}
