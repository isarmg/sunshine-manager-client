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
use serde_json::Value;
use sha2::{Digest, Sha256};
use sunshine_client_protocol::{ConfigSnapshot, Effectiveness, config::FIELDS};
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
}

/// Full configuration stays on the Client. Deliberately not Debug or Serialize.
#[derive(Clone, PartialEq, Eq)]
pub struct Configuration {
    sunshine_version: String,
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
        if !response
            .remove("platform")
            .is_some_and(|value| value.is_string())
        {
            return Err(AdapterError::UnsafeConfiguration);
        }
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
                || (FIELDS.contains(&key.as_str()) && value.len() > 512)
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

    pub fn snapshot(&self, effectiveness: Effectiveness) -> ConfigSnapshot {
        ConfigSnapshot {
            revision: self.revision(),
            sunshine_version: self.sunshine_version.clone(),
            fields: self
                .fields
                .iter()
                .filter(|(key, _)| FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
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
            if !FIELDS.contains(&key.as_str()) {
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
}

/// Only loopback-literal HTTPS; caller supplies a locally provisioned trust anchor.
/// No proxy, redirect, arbitrary paths, insecure TLS option, or network credentials in DTOs.
pub struct LocalSunshine {
    client: LocalClient,
    base: Url,
    authorization: header::HeaderValue,
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
        })
    }

    async fn request(
        &self,
        method: Method,
        path: &'static str,
        body: Option<Vec<u8>>,
    ) -> Result<Value, AdapterError> {
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
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AdapterError::UnsafeConfiguration)?;
        if value.get("status") != Some(&Value::Bool(true)) {
            return Err(AdapterError::ApiUnavailable);
        }
        Ok(value)
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
