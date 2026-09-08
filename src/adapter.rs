use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Method, Request, header};
use sarmg_client_secure_http::{
    Certificate, NetworkPolicy, ResponseBudget, SecureHttpClient, TlsConfig, Url,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sunshine_client_protocol::{ConfigSnapshot, Effectiveness, SUNSHINE_VERSION, config::FIELDS};
use zeroize::Zeroizing;

pub const MAX_CONFIG_BYTES: usize = 512 * 1024;

/// Never print upstream errors/bodies: they may include local settings or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("local Sunshine HTTPS request failed")]
    Unavailable,
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
    fields: BTreeMap<String, String>,
}

impl Configuration {
    pub fn from_response(response: Value) -> Result<Self, AdapterError> {
        let mut response = response
            .as_object()
            .cloned()
            .ok_or(AdapterError::UnsafeConfiguration)?;
        if response.remove("status") != Some(Value::Bool(true)) {
            return Err(AdapterError::Unavailable);
        }
        if response.remove("version").as_ref().and_then(Value::as_str) != Some(SUNSHINE_VERSION) {
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
        let configuration = Self { fields };
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

    pub fn snapshot(&self, effectiveness: Effectiveness) -> ConfigSnapshot {
        ConfigSnapshot {
            revision: self.revision(),
            sunshine_version: SUNSHINE_VERSION.to_owned(),
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
    client: SecureHttpClient,
    base: Url,
    authorization: header::HeaderValue,
}

impl LocalSunshine {
    pub fn new(
        endpoint: &str,
        username: &str,
        password: Zeroizing<String>,
        root_pem: &[u8],
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
        let roots = if root_pem.is_empty() {
            Vec::new()
        } else {
            vec![Certificate::from_pem(root_pem).map_err(|_| AdapterError::InvalidLocalEndpoint)?]
        };
        let client = SecureHttpClient::new(
            Duration::from_secs(15),
            ResponseBudget {
                max_header_bytes: 16 * 1024,
                max_body_bytes: MAX_CONFIG_BYTES,
            },
            TlsConfig {
                identity: None,
                roots,
            },
            format!("sunshine-client/{}", env!("CARGO_PKG_VERSION")),
        )
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
        let response = self
            .client
            .execute(
                NetworkPolicy::PrivateDevice {
                    allow_loopback: true,
                    allow_link_local: false,
                },
                request,
            )
            .await
            .map_err(|_| AdapterError::Unavailable)?;
        if !response.status.is_success() {
            return Err(AdapterError::Unavailable);
        }
        let value: Value = serde_json::from_slice(&response.body)
            .map_err(|_| AdapterError::UnsafeConfiguration)?;
        if value.get("status") != Some(&Value::Bool(true)) {
            return Err(AdapterError::Unavailable);
        }
        Ok(value)
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
