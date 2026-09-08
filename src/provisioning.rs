//! Explicit local provisioning. No secret command-line arguments or remote installation.
use crate::{
    adapter::{LocalSunshine, Sunshine},
    engine::Executor,
    journal::FileJournal,
    storage::{ProtectedState, StorageError},
    transport::{HealthObservation, ManagerConnection, TransportError},
};
use rand::RngCore;
use sarmg_client_secure_http::{NetworkPolicy, ResponseBudget, SecureHttpClient, TlsConfig, Url};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use sunshine_client_protocol::{
    Binding, Capabilities, ClientOs, Effectiveness, PROTOCOL, SUNSHINE_VERSION, config::FIELDS,
};
use tokio::sync::watch;
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
    #[serde(default)]
    pub restart_allowed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    binding: Binding,
    credential: Zeroizing<String>,
    config: Bootstrap,
    enrolled: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("invalid protected bootstrap configuration")]
    Configuration,
    #[error("Manager enrollment unavailable; retained identity allows a safe retry")]
    Unavailable,
    #[error("device credential or enrollment ticket rejected; local administrator action required")]
    Rejected,
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("Client execution journal unavailable")]
    Journal,
}
fn config_error(_: impl std::fmt::Debug) -> ProvisionError {
    ProvisionError::Configuration
}
/// Validate locally before accepting a bootstrap; never print its secret fields.
pub(crate) fn validate_bootstrap(bytes: &[u8]) -> Result<(), ProvisionError> {
    let config: Bootstrap = serde_json::from_slice(bytes).map_err(config_error)?;
    config.validate()
}
pub(crate) fn pending_retry(config: &[u8], identity: &[u8]) -> bool {
    let Ok(incoming) = serde_json::from_slice::<Bootstrap>(config) else {
        return false;
    };
    let Ok(existing) = serde_json::from_slice::<Identity>(identity) else {
        return false;
    };
    !existing.enrolled && incoming == existing.config
}

impl Bootstrap {
    fn validate(&self) -> Result<(), ProvisionError> {
        if self.enrollment_token.len() != 64 {
            return Err(ProvisionError::Configuration);
        }
        ManagerConnection::new(&self.manager_endpoint, Zeroizing::new("a".repeat(64)), &[])?;
        self.adapter()?;
        Ok(())
    }
    fn adapter(&self) -> Result<LocalSunshine, ProvisionError> {
        LocalSunshine::new(
            &self.sunshine_endpoint,
            &self.sunshine_username,
            self.sunshine_password.clone(),
            &[],
        )
        .map_err(config_error)
    }
}
impl Identity {
    fn persist(&self, store: &ProtectedState) -> Result<(), ProvisionError> {
        let bytes = Zeroizing::new(serde_json::to_vec(self).map_err(config_error)?);
        store.put("identity.json", &bytes)?;
        Ok(())
    }
}

fn system_client() -> Result<SecureHttpClient, ProvisionError> {
    SecureHttpClient::new(
        Duration::from_secs(15),
        ResponseBudget {
            max_header_bytes: 16384,
            max_body_bytes: 16384,
        },
        TlsConfig::default(),
        format!("sunshine-client/{}", env!("CARGO_PKG_VERSION")),
    )
    .map_err(config_error)
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
    endpoint.set_path("/sunshine-client/v1/pairing");
    let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
    request.headers_mut().insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    *request.body_mut() = Some(
        serde_json::to_vec(&serde_json::json!({"token":config.enrollment_token.as_str()}))
            .map_err(config_error)?
            .into(),
    );
    let response = system_client()?
        .execute(
            NetworkPolicy::PrivateDevice {
                allow_loopback: true,
                allow_link_local: false,
            },
            request,
        )
        .await
        .map_err(|_| ProvisionError::Unavailable)?;
    if !response.status.is_success() {
        return Err(ProvisionError::Rejected);
    }
    let target: PairingTarget = serde_json::from_slice(&response.body).map_err(config_error)?;
    if target.manager_id.is_nil() || target.device_id.is_nil() {
        return Err(ProvisionError::Configuration);
    }
    Ok(target)
}
/// Persist independent random identity before enrollment. Never regenerate on retry.
async fn provision(store: &ProtectedState) -> Result<Identity, ProvisionError> {
    let mut identity = if let Some(bytes) = store.read("identity.json")? {
        serde_json::from_slice::<Identity>(&Zeroizing::new(bytes)).map_err(config_error)?
    } else {
        let bytes = Zeroizing::new(
            store
                .read("bootstrap.json")?
                .ok_or(ProvisionError::Configuration)?,
        );
        let config: Bootstrap = serde_json::from_slice(&bytes).map_err(config_error)?;
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
        };
        value.persist(store)?;
        value
    };
    if identity.enrolled {
        return Ok(identity);
    }
    let mut endpoint = Url::parse(&identity.config.manager_endpoint).map_err(config_error)?;
    endpoint.set_scheme("https").map_err(config_error)?;
    endpoint.set_path("/sunshine-client/v1/identity");
    let client = system_client()?;
    let policy = NetworkPolicy::PrivateDevice {
        allow_loopback: true,
        allow_link_local: false,
    };
    let mut request = reqwest::Request::new(reqwest::Method::GET, endpoint.clone());
    let raw = Zeroizing::new(format!("Bearer {}", identity.credential.as_str()));
    let mut header = reqwest::header::HeaderValue::from_str(&raw).map_err(config_error)?;
    header.set_sensitive(true);
    request
        .headers_mut()
        .insert(reqwest::header::AUTHORIZATION, header);
    let response = client
        .execute(policy, request)
        .await
        .map_err(|_| ProvisionError::Unavailable)?;
    let binding: Binding = if response.status.is_success() {
        serde_json::from_slice(&response.body).map_err(config_error)?
    } else if response.status == reqwest::StatusCode::UNAUTHORIZED {
        endpoint.set_path("/sunshine-client/v1/enroll");
        let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
        request.headers_mut().insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        // The body is sent only to the locally pinned TLS endpoint and never logged.
        *request.body_mut()=Some(serde_json::to_vec(&serde_json::json!({"device_id":identity.binding.device_id,"installation_id":identity.binding.installation_id,"token":identity.config.enrollment_token.as_str(),"credential":identity.credential.as_str()})).map_err(config_error)?.into());
        let response = client
            .execute(policy, request)
            .await
            .map_err(|_| ProvisionError::Unavailable)?;
        if matches!(response.status.as_u16(), 401 | 403) {
            return Err(ProvisionError::Rejected);
        }
        if !response.status.is_success() {
            return Err(ProvisionError::Unavailable);
        }
        serde_json::from_slice(&response.body).map_err(config_error)?
    } else {
        return Err(ProvisionError::Rejected);
    };
    if binding != identity.binding {
        return Err(ProvisionError::Configuration);
    }
    identity.enrolled = true;
    identity.config.enrollment_token.clear();
    identity.persist(store)?;
    store.put("bootstrap.json", b"{}")?;
    Ok(identity)
}

/// Local wizard completion requires a verified Sunshine read and successful enrollment.
pub async fn pair(state_path: &Path) -> Result<(), ProvisionError> {
    let _root = crate::storage::prepare_root(state_path)?;
    let store = ProtectedState::open(&state_path.join("provisioning"))?;
    let config: Bootstrap = if let Some(bytes) = store.read("identity.json")? {
        serde_json::from_slice::<Identity>(&Zeroizing::new(bytes))
            .map_err(config_error)?
            .config
    } else {
        serde_json::from_slice(&Zeroizing::new(
            store
                .read("bootstrap.json")?
                .ok_or(ProvisionError::Configuration)?,
        ))
        .map_err(config_error)?
    };
    config.adapter()?.read().await.map_err(config_error)?;
    provision(&store).await?;
    Ok(())
}
pub async fn run(state_path: &Path, shutdown: watch::Receiver<bool>) -> Result<(), ProvisionError> {
    let _root = crate::storage::prepare_root(state_path)?;
    let store = ProtectedState::open(&state_path.join("provisioning"))?;
    let mut stopping = shutdown.clone();
    let identity = loop {
        if *stopping.borrow() {
            return Ok(());
        }
        match provision(&store).await {
            Ok(identity) => break identity,
            Err(ProvisionError::Unavailable) => {
                tokio::select! {_=stopping.changed()=>{if *stopping.borrow(){return Ok(());}},_=tokio::time::sleep(Duration::from_secs(10))=>{}}
            }
            Err(error) => return Err(error),
        }
    };
    let journal =
        FileJournal::open(&state_path.join("journal")).map_err(|_| ProvisionError::Journal)?;
    let executor = Arc::new(Executor::new(
        identity.binding.clone(),
        identity.config.restart_allowed,
        identity.config.adapter()?,
        journal,
    ));
    let connection =
        ManagerConnection::new(&identity.config.manager_endpoint, identity.credential, &[])?;
    let capabilities = Capabilities {
        protocol: PROTOCOL.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        os: if cfg!(target_os = "windows") {
            ClientOs::WindowsX86_64
        } else {
            ClientOs::LinuxX86_64
        },
        sunshine_version: SUNSHINE_VERSION.into(),
        restart_allowed: identity.config.restart_allowed,
        managed_fields: FIELDS.iter().map(|s| s.to_string()).collect(),
    };
    let (health_tx, health_rx) = watch::channel(HealthObservation::default());
    let mut adapter = identity.config.adapter()?;
    // Read-only health probes do not share the mutation lane and survive a Sunshine restart.
    let monitor = async move {
        loop {
            let read = adapter.read().await;
            let observation = HealthObservation {
                sunshine_reachable: read.is_ok(),
                configuration: read
                    .ok()
                    .map(|c| c.snapshot(Effectiveness::PendingVerification)),
                observed_at: Some(tokio::time::Instant::now()),
            };
            if health_tx.send(observation).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    };
    tokio::select! {
        result=connection.run(identity.binding,capabilities,executor,health_rx,shutdown)=>result.map_err(ProvisionError::from),
        _=monitor=>Err(ProvisionError::Unavailable),
    }
}

#[cfg(test)]
mod bootstrap_tests {
    fn configuration() -> serde_json::Value {
        serde_json::json!({"manager_endpoint":"wss://manager.example.org/sunshine-client/v1/connect", "enrollment_token":"a".repeat(64),
            "sunshine_endpoint":"https://127.0.0.1:47990/", "sunshine_username":"fixture", "sunshine_password":"local-only", "restart_allowed":false})
    }
    #[test]
    fn bootstrap_only_accepts_system_trust_and_server_resolved_binding() {
        let config = configuration();
        assert!(super::validate_bootstrap(&serde_json::to_vec(&config).unwrap()).is_ok());
        for field in [
            "manager_ca_pem",
            "sunshine_ca_pem",
            "manager_id",
            "device_id",
        ] {
            let mut extra = config.clone();
            extra[field] = serde_json::json!("not permitted");
            assert!(super::validate_bootstrap(&serde_json::to_vec(&extra).unwrap()).is_err());
        }
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
        identity["config"]["enrollment_token"] = serde_json::json!("c".repeat(64));
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
