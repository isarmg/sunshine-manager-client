use crate::{
    provisioning::{self, Bootstrap, Identity, ProvisionError},
    storage::{MaintenanceGuard, ProtectedState},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use xcsc::cli::*;
use zeroize::Zeroizing;

const MAX_URL_BYTES: usize = 2_048;
const MAX_AUTHORIZATION_CODE_BYTES: usize = xscs_protocol::AUTHORIZATION_CODE_LENGTH;
const MAX_SUNSHINE_USERNAME_BYTES: usize = 256;
const MAX_SUNSHINE_PASSWORD_BYTES: usize = 4_096;
const MAX_CONFIRMATION_BYTES: usize = 16;

struct XsccErrorCatalog;

impl ProductErrorCatalog for XsccErrorCatalog {
    fn message(&self, code: &'static str) -> Option<&'static str> {
        match code {
            "awaiting_configuration" => Some("xscc has not been configured."),
            "awaiting_pairing" => Some("xscc has not completed Manager pairing."),
            "active_setup_input_requires_pair_replace"
            | "binding_replacement_requires_pair_replace" => Some(
                "This xscc is already bound; replacement requires the explicit pair replace workflow.",
            ),
            "credential_rejected" => {
                Some("xscs rejected the instance authorization code or client credential.")
            }
            "client_credential_rejected" => {
                Some("xscs rejected the saved client credential or no longer has this binding.")
            }
            "invalid_server_origin" => Some("The xscs address must be a valid HTTPS origin."),
            "invalid_authorization_code" => Some(
                "The Sunshine instance authorization code must be the exact 36-character code issued by the Manager.",
            ),
            "no_pairing_transaction" => {
                Some("There is no saved xscs pairing transaction to resume.")
            }
            "pairing_endpoint_not_found" => {
                Some("The configured xscs does not expose the required pairing endpoint.")
            }
            "pairing_http_method_rejected" => {
                Some("xscs or its reverse proxy rejected the pairing HTTP method.")
            }
            "pairing_server_upgrade_required" => {
                Some("xscs requires a different current pairing contract.")
            }
            "pairing_request_rejected" => Some("xscs rejected the pairing request."),
            "pairing_unexpected_http_status" => {
                Some("xscs returned an unexpected pairing HTTP status.")
            }
            "unsupported_protocol_or_platform" => {
                Some("xscc and Manager do not support the same current protocol or platform.")
            }
            "server_unavailable" => {
                Some("xscs could not be reached or its TLS identity could not be trusted.")
            }
            "pairing_postcondition_unconfirmed" | "pairing_result_uncertain" => {
                Some("Manager pairing returned without a durable active xscc identity.")
            }
            "server_replacement_requires_pair_replace"
            | "server_replacement_requires_retirement" => {
                Some("Changing xscs requires the explicit binding replacement workflow.")
            }
            "invalid_sunshine_configuration" => {
                Some("Local Sunshine must use an HTTPS loopback IP and valid local credentials.")
            }
            "sunshine_credentials_rejected" => {
                Some("Local Sunshine rejected its Basic Authentication credentials.")
            }
            "sunshine_api_unavailable" | "sunshine_probe_timeout" => {
                Some("The local loopback Sunshine HTTPS API is unavailable.")
            }
            "network_probe_timeout" => {
                Some("The xscs network probe exceeded the requested timeout.")
            }
            "sunshine_version_unsupported" => {
                Some("The installed local Sunshine version is unsupported.")
            }
            "pairing_state_incompatible" => {
                Some("The stored xscc account or pairing data cannot be used by this version.")
            }
            "important_state_incompatible" => Some(
                "Important saved Sunshine execution data is incompatible or unreadable and was preserved.",
            ),
            "sunshine_resource_conflict" => {
                Some("Local Sunshine changed while the requested resource was being updated.")
            }
            "sunshine_capability_unavailable" => {
                Some("Local Sunshine does not expose the requested management capability.")
            }
            "service_state_unconfirmed" => Some(
                "The registered xscc service did not remain running; it may have exited during startup.",
            ),
            _ => None,
        }
    }

    fn next_step(&self, product: &str, error: &Failure) -> Option<String> {
        match error.code {
            "awaiting_configuration" | "awaiting_pairing" | "no_pairing_transaction" => Some(
                format!("Run `{product} setup --interactive` to configure and pair this client."),
            ),
            "credential_rejected" => Some(format!(
                "Create or rotate this Sunshine instance authorization code, then run `{product} setup --interactive`."
            )),
            "client_credential_rejected" => Some(format!(
                "Create a new instance authorization code, then run `{product} pair replace --interactive`."
            )),
            "invalid_server_origin"
            | "invalid_authorization_code"
            | "pairing_postcondition_unconfirmed"
            | "pairing_result_uncertain" => Some(format!(
                "Check the Manager address and instance code, then run `{product} setup --interactive`."
            )),
            "server_unavailable" | "network_probe_timeout" => Some(format!(
                "Check the xscs URL, TLS certificate and network, then retry `{product} setup`."
            )),
            "pairing_endpoint_not_found" | "pairing_http_method_rejected" => Some(
                "Check the xscs address and reverse-proxy routing, then retry Setup."
                    .into(),
            ),
            "pairing_server_upgrade_required" | "unsupported_protocol_or_platform" => Some(
                "Upgrade the older xscc or Manager to the same current contract.".into(),
            ),
            "invalid_sunshine_configuration"
            | "sunshine_credentials_rejected"
            | "sunshine_api_unavailable"
            | "sunshine_probe_timeout" => Some(format!(
                "Check the local loopback Sunshine HTTPS endpoint and credentials, then run `{product} doctor --sunshine`."
            )),
            "sunshine_version_unsupported" | "sunshine_capability_unavailable" => {
                Some("Upgrade local Sunshine to a supported current version.".into())
            }
            "service_state_unconfirmed" => Some(format!(
                "Run `sudo {product} logs --tail 100`, then `sudo {product} service status --format json` to inspect the startup failure."
            )),
            "pairing_state_incompatible" => Some(format!(
                "Create a new Sunshine instance authorization code, then run `{product} pair replace --interactive`; the incompatible account document will be archived."
            )),
            "important_state_incompatible" => Some(
                "Do not delete or replace the reported execution journal; restore it with a compatible Client or archive it for operator review."
                    .into(),
            ),
            _ => None,
        }
    }
}

fn emit_xscc(command: &str, format: &str, result: &Result<Value>) -> u8 {
    emit("xscc", command, format, result, &XsccErrorCatalog)
}

fn default_state() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()))
            .join("Xscc")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/xscc")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/var/lib/xscc")
    }
}
fn service() -> Service {
    Service {
        #[cfg(not(target_os = "macos"))]
        name: if cfg!(windows) {
            "Xscc"
        } else {
            "xscc.service"
        },
        label: "org.sarmg.xscc",
        default_config: default_state(),
        binary: "xscc",
        log_path: "/var/log/xscc.log",
    }
}
fn storage_error(error: impl std::fmt::Debug + std::any::Any) -> Failure {
    match crate::storage::storage_error(error) {
        crate::storage::StorageError::PermissionDenied => fail(3, "permission_denied"),
        crate::storage::StorageError::Busy => fail(5, "busy"),
        crate::storage::StorageError::Published => {
            let mut error = fail(11, "state_committed_durability_unconfirmed");
            error.committed = true;
            error
        }
        crate::storage::StorageError::Unsafe => fail(8, "unsafe_or_corrupt_state"),
        crate::storage::StorageError::DocumentCorrupt { artifact } => {
            fail(4, "pairing_state_incompatible").with_detail(format!(
                "artifact={artifact};detected=malformed;supported=current-structure;preserved=true"
            ))
        }
    }
}
fn provision_error(e: ProvisionError) -> Failure {
    match e {
        ProvisionError::Configuration => fail(2, "invalid_configuration"),
        ProvisionError::EntropyUnavailable => fail(6, "system_entropy_unavailable"),
        ProvisionError::InvalidAuthorizationCode => fail(2, "invalid_authorization_code"),
        ProvisionError::StateDocumentCorrupt { artifact } => fail(4, "pairing_state_incompatible")
            .with_detail(format!(
                "artifact={artifact};detected=malformed;supported=current-structure;preserved=true"
            )),
        ProvisionError::Rejected => fail(7, "credential_rejected"),
        ProvisionError::Unavailable | ProvisionError::RateLimited => fail(6, "server_unavailable"),
        ProvisionError::Unsupported => fail(10, "unsupported_protocol_or_platform"),
        ProvisionError::PairingEndpointNotFound => fail(10, "pairing_endpoint_not_found"),
        ProvisionError::PairingHttpMethodRejected => fail(10, "pairing_http_method_rejected"),
        ProvisionError::PairingServerUpgradeRequired => fail(10, "pairing_server_upgrade_required"),
        ProvisionError::PairingRequestRejected => fail(2, "pairing_request_rejected"),
        ProvisionError::PairingUnexpectedHttpStatus => fail(10, "pairing_unexpected_http_status"),
        ProvisionError::SunshineCredentialsRejected => fail(7, "sunshine_credentials_rejected"),
        ProvisionError::SunshineApiUnavailable => fail(6, "sunshine_api_unavailable"),
        ProvisionError::SunshineVersionUnsupported { detected, origin } => {
            let source = match origin {
                crate::adapter::VersionSource::ConfigurationResponse => "configuration_response",
                crate::adapter::VersionSource::HttpUpgradeRequired => "http_upgrade_required",
            };
            fail(10, "sunshine_version_unsupported").with_detail(format!(
                "detected={};supported={};source={source}",
                detected.as_deref().unwrap_or("unknown"),
                xscs_protocol::SUPPORTED_SUNSHINE_VERSIONS.join(",")
            ))
        }
        ProvisionError::Unpaired => fail(4, "awaiting_pairing"),
        ProvisionError::Transport(error) => match error {
            crate::transport::TransportError::Configuration => fail(2, "invalid_configuration"),
            crate::transport::TransportError::Diagnostics => fail(10, "diagnostics_unavailable"),
            crate::transport::TransportError::Revoked => fail(7, "credential_rejected"),
            crate::transport::TransportError::Disconnected => fail(6, "server_unavailable"),
            crate::transport::TransportError::Protocol => {
                fail(10, "unsupported_protocol_or_platform")
            }
        },
        ProvisionError::Storage(error) => storage_error(error),
        ProvisionError::Journal => important_state_error("execution-journal"),
    }
}

fn important_state_error(artifact: &'static str) -> Failure {
    fail(10, "important_state_incompatible")
        .with_detail(format!("artifact={artifact};preserved=true"))
}

fn journal_error(_: crate::engine::JournalError) -> Failure {
    important_state_error("execution-journal")
}

fn ensure_execution_journal_compatible(path: &Path) -> Result<()> {
    let journal = path.join("journal");
    match std::fs::symlink_metadata(&journal) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(important_state_error("execution-journal")),
        Ok(_) => crate::journal::inspect(&journal, None)
            .map(|_| ())
            .map_err(journal_error),
    }
}

fn incompatible_identity_archive_name() -> String {
    format!(
        "identity.incompatible-{}.json",
        uuid::Uuid::new_v4().simple()
    )
}

fn sunshine_adapter_error(error: crate::adapter::AdapterError) -> Failure {
    match error {
        crate::adapter::AdapterError::CredentialsRejected => {
            fail(7, "sunshine_credentials_rejected").with_detail(
                "tcp=connected tls=encrypted certificate_identity=not_checked_local_loopback_policy api=available credentials=rejected version=not_checked",
            )
        }
        crate::adapter::AdapterError::ApiUnavailable => fail(6, "sunshine_api_unavailable")
            .with_detail(
                "tcp=unknown tls=unknown api=unavailable credentials=unknown version=not_checked",
            ),
        crate::adapter::AdapterError::UnsupportedVersion { detected, origin } => {
            let source = match origin {
                crate::adapter::VersionSource::ConfigurationResponse => "configuration_response",
                crate::adapter::VersionSource::HttpUpgradeRequired => "http_upgrade_required",
            };
            fail(10, "sunshine_version_unsupported").with_detail(format!(
                "tcp=connected tls=encrypted certificate_identity=not_checked_local_loopback_policy api=available credentials=accepted detected={};supported={};source={source}",
                detected.as_deref().unwrap_or("unknown"),
                xscs_protocol::SUPPORTED_SUNSHINE_VERSIONS.join(",")
            ))
        }
        crate::adapter::AdapterError::UnsafeConfiguration
        | crate::adapter::AdapterError::InvalidLocalEndpoint => {
            fail(2, "invalid_sunshine_configuration")
        }
        crate::adapter::AdapterError::ResourceConflict => fail(9, "sunshine_resource_conflict"),
        crate::adapter::AdapterError::PairingRejected => fail(9, "moonlight_pairing_failed"),
        crate::adapter::AdapterError::UnsupportedCapability => {
            fail(10, "sunshine_capability_unavailable")
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairInput {
    server: String,
    authorization_code: Zeroizing<String>,
    sunshine_endpoint: String,
    sunshine_username: Zeroizing<String>,
    sunshine_password: Zeroizing<String>,
}
impl PairInput {
    fn bootstrap(self) -> Result<Bootstrap> {
        let mut url = url::Url::parse(&self.server).map_err(input_error)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(fail(2, "invalid_server_origin"));
        }
        url.set_scheme("wss").map_err(input_error)?;
        url.set_path(crate::transport::CONNECT_PATH);
        let b = Bootstrap {
            manager_endpoint: url.to_string(),
            enrollment_token: self.authorization_code,
            sunshine_endpoint: self.sunshine_endpoint,
            sunshine_username: self.sunshine_username,
            sunshine_password: self.sunshine_password,
        };
        b.validate().map_err(provision_error)?;
        Ok(b)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    sunshine_endpoint: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    sunshine_username: Zeroizing<String>,
    sunshine_password: Zeroizing<String>,
}
fn read_store(path: &Path) -> Result<Option<ProtectedState>> {
    match std::fs::symlink_metadata(path.join("provisioning")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(storage_error(e)),
        Ok(_) => ProtectedState::open_readonly(&path.join("provisioning"))
            .map(Some)
            .map_err(storage_error),
    }
}
fn identity(store: &ProtectedState) -> Result<Option<Identity>> {
    store
        .read("identity.json")
        .map_err(storage_error)?
        .map(|b| provisioning::decode_identity(&Zeroizing::new(b)).map_err(provision_error))
        .transpose()
}
fn current(store: &ProtectedState) -> Result<(Bootstrap, Option<Identity>)> {
    if let Some(id) = identity(store)? {
        let b = serde_json::from_value(serde_json::to_value(&id.config).map_err(storage_error)?)
            .map_err(storage_error)?;
        Ok((b, Some(id)))
    } else if store
        .read("bootstrap.json")
        .map_err(storage_error)?
        .is_none()
    {
        let bytes = store
            .read("local-settings.json")
            .map_err(storage_error)?
            .ok_or_else(|| fail(4, "awaiting_configuration"))?;
        let settings: Settings = serde_json::from_slice(&bytes).map_err(storage_error)?;
        Ok((
            Bootstrap {
                manager_endpoint: String::new(),
                enrollment_token: Zeroizing::new(String::new()),
                sunshine_endpoint: settings.sunshine_endpoint,
                sunshine_username: Zeroizing::new(String::new()),
                sunshine_password: Zeroizing::new(String::new()),
            },
            None,
        ))
    } else {
        let bytes = Zeroizing::new(
            store
                .read("bootstrap.json")
                .map_err(storage_error)?
                .ok_or_else(|| fail(4, "awaiting_configuration"))?,
        );
        Ok((
            provisioning::decode_bootstrap(&bytes).map_err(provision_error)?,
            None,
        ))
    }
}
fn settings(b: &Bootstrap) -> Settings {
    Settings {
        sunshine_endpoint: b.sunshine_endpoint.clone(),
    }
}
pub(crate) fn settings_revision(b: &Bootstrap) -> Result<String> {
    Ok(revision(
        &serde_json::to_vec(&settings(b)).map_err(storage_error)?,
    ))
}
fn local_status(path: &Path) -> Result<Value> {
    let Some(store) = read_store(path)? else {
        return Ok(
            json!({"pairing":{"state":"unconfigured"},"health":"unknown","runtime":{"available":false}}),
        );
    };
    let (config, id) = current(&store)?;
    let revision = settings_revision(&config)?;
    let pairing = if let Some(id) = id {
        json!({"state":if id.enrolled {"active"}else{"pending"},"binding":id.binding,"transaction_id":id.binding.installation_id})
    } else {
        json!({"state":"awaiting_pairing"})
    };
    let runtime = crate::runtime_status::read(path, pairing["binding"]["installation_id"].as_str());
    let effective = runtime.as_ref().map(|s| s["effective_revision"].clone());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let healthy = runtime.as_ref().is_some_and(|s| {
        s["manager_session"] == "connected"
            && s["last_peer_at"]
                .as_u64()
                .is_some_and(|at| at <= now && now - at < 45)
            && s["sunshine_reachable"] == true
            && s["sunshine_observed_at"]
                .as_u64()
                .is_some_and(|at| at <= now && now - at < 30)
            && s["effective_revision"] == revision
    });
    Ok(
        json!({"pairing":pairing,"config":{"stored_revision":revision,"effective_revision":effective,"restart_required":effective.as_ref().map(|r|r!=&json!(revision))},"runtime":runtime.unwrap_or(json!({"available":false,"reason":"runtime_summary_unavailable"})),"health":if healthy {"healthy"}else{"unknown"}}),
    )
}
fn document<T: serde::de::DeserializeOwned + Send + 'static>(args: &Args) -> Result<T> {
    if args.has("--input-stdin") {
        if args.has("--file") {
            return Err(fail(2, "conflicting_input_sources"));
        }
        stdin_document(args.timeout)
    } else if let Some(path) = args.get("--file") {
        let bytes = crate::storage::read_input(Path::new(path)).map_err(storage_error)?;
        serde_json::from_slice(&bytes).map_err(input_error)
    } else {
        Err(fail(2, "protected_input_required"))
    }
}

fn ask_yes_no(label: &str, default: bool, deadline: Instant) -> Result<bool> {
    let value = prompt_text(
        &format!("{label} [{}]", if default { "yes" } else { "no" }),
        MAX_CONFIRMATION_BYTES,
        deadline,
    )?;
    confirmation_answer(&value, default)
}

fn confirmation_answer(value: &str, default: bool) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "y" | "yes" | "true" | "1" => Ok(true),
        "n" | "no" | "false" | "0" => Ok(false),
        _ => Err(fail(2, "invalid_confirmation")),
    }
}

fn setup_autostart(
    interactive: bool,
    ask: impl FnOnce(&str, bool) -> Result<bool>,
) -> Result<bool> {
    if interactive {
        ask("Enable the service at startup?", true)
    } else {
        Ok(true)
    }
}

fn setup_args(args: &Args, words: Vec<String>, interactive: bool) -> Args {
    let mut options: std::collections::BTreeMap<String, String> = args
        .options
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "--format"
                    | "--timeout"
                    | "--config"
                    | "--state"
                    | "--non-interactive"
                    | "--no-color"
                    | "--input-stdin"
                    | "--server"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if interactive {
        options.insert("--interactive".into(), "true".into());
    }
    Args {
        words,
        options,
        format: args.format.clone(),
        timeout: args.timeout,
    }
}

fn setup_service_args(args: &Args) -> Args {
    let options = args
        .options
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "--format"
                    | "--timeout"
                    | "--config"
                    | "--state"
                    | "--non-interactive"
                    | "--no-color"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Args {
        words: vec![],
        options,
        format: args.format.clone(),
        timeout: args.timeout,
    }
}

fn preserve_setup_commit(mut error: Failure, pairing: &Value) -> Failure {
    error.committed = true;
    if error.transaction_id.is_none() {
        error.transaction_id = pairing["pairing"]["transaction_id"]
            .as_str()
            .or_else(|| pairing["transaction_id"].as_str())
            .map(str::to_owned);
    }
    error
}

fn setup_failure(error: Failure, pairing: &Value, step: &'static str) -> Failure {
    preserve_setup_commit(error, pairing).at_step(step)
}

fn startup_policy_matches(status: &Value, enabled: bool) -> bool {
    let observed = status["startup"].as_str().unwrap_or("unknown");
    if enabled {
        matches!(observed, "automatic" | "enabled")
    } else {
        matches!(observed, "manual" | "disabled")
    }
}

fn pairing_is_active(status: &Value) -> bool {
    status["pairing"]["state"] == "active"
}

fn record_setup_step(
    steps: &mut Vec<Value>,
    interactive: bool,
    step: &'static str,
    status: &'static str,
    evidence: Value,
) {
    if interactive {
        eprintln!("[setup] {step}: {status}");
    }
    steps.push(json!({"step":step,"status":status,"evidence":evidence}));
}

fn wait_for_healthy(path: &Path, timeout: std::time::Duration) -> Result<Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let status = local_status(path)?;
        if status["health"] == "healthy" {
            return Ok(status);
        }
        if status["runtime"]["last_error_code"] == "credential_rejected" {
            return Err(fail(7, "client_credential_rejected"));
        }
        if status["runtime"]["last_error_code"] == "unsupported_protocol_or_platform" {
            return Err(fail(10, "unsupported_protocol_or_platform"));
        }
        if std::time::Instant::now() >= deadline {
            return Err(fail(9, "connection_unconfirmed"));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn execute_pair(args: &Args, path: &Path) -> Result<Value> {
    let deadline = Instant::now() + args.timeout;
    let words: Vec<_> = args.words.iter().map(String::as_str).collect();
    args.validate_options(&["--interactive", "--input-stdin", "--server"])?;
    let resume = words == ["pair", "resume"];
    let replace = words == ["pair", "replace"];
    if resume && (args.has("--interactive") || args.has("--input-stdin") || args.has("--server")) {
        return Err(fail(2, "resume_uses_existing_transaction"));
    }
    let incoming = if args.has("--interactive") {
        let server = if let Some(server) = args.get("--server") {
            server.into()
        } else {
            prompt_text("Server HTTPS origin", MAX_URL_BYTES, deadline)?
        };
        let authorization_code = Zeroizing::new(prompt_text(
            "Authorization code (visible)",
            MAX_AUTHORIZATION_CODE_BYTES,
            deadline,
        )?);
        if !xscs_protocol::is_valid_authorization_code(&authorization_code) {
            return Err(fail(2, "invalid_authorization_code"));
        }
        Some(
            PairInput {
                server,
                authorization_code,
                sunshine_endpoint: prompt_text(
                    "Local Sunshine HTTPS URL",
                    MAX_URL_BYTES,
                    deadline,
                )?,
                sunshine_username: Zeroizing::new(prompt_text(
                    "Sunshine username",
                    MAX_SUNSHINE_USERNAME_BYTES,
                    deadline,
                )?),
                sunshine_password: prompt_secret(
                    "Sunshine password",
                    MAX_SUNSHINE_PASSWORD_BYTES,
                    deadline,
                )?,
            }
            .bootstrap()?,
        )
    } else if args.has("--input-stdin") {
        let input: PairInput = stdin_document(args.timeout)?;
        if args.get("--server").is_some_and(|s| s != input.server) {
            return Err(fail(2, "server_input_mismatch"));
        }
        Some(input.bootstrap()?)
    } else {
        None
    };
    if replace && incoming.is_none() {
        return Err(fail(2, "replacement_requires_protected_input"));
    }
    let _guard = MaintenanceGuard::acquire(path).map_err(storage_error)?;
    let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
    let mut archived_incompatible_identity = false;
    let existing = match identity(&store) {
        Ok(existing) => existing,
        Err(error) if replace && error.code == "pairing_state_incompatible" => {
            ensure_execution_journal_compatible(path)?;
            store
                .archive("identity.json", &incompatible_identity_archive_name())
                .map_err(storage_error)?;
            archived_incompatible_identity = true;
            None
        }
        Err(error) => return Err(error),
    };
    if resume && existing.is_none() {
        return Err(fail(4, "no_pairing_transaction"));
    }
    if let Some(b) = incoming {
        if replace {
            if archived_incompatible_identity {
                store
                    .put(
                        "bootstrap.json",
                        &Zeroizing::new(serde_json::to_vec(&b).map_err(storage_error)?),
                    )
                    .map_err(storage_error)?;
            } else {
                let mut id = existing.ok_or_else(|| fail(4, "no_existing_binding"))?;
                if !id.enrolled {
                    return Err(fail(5, "pending_pairing_must_be_resumed"));
                }
                if id.config.manager_endpoint != b.manager_endpoint {
                    return Err(fail(5, "server_replacement_requires_retirement"));
                }
                tokio::runtime::Runtime::new()
                    .map_err(storage_error)?
                    .block_on(provisioning::validate_replacement(&b, &id.binding))
                    .map_err(provision_error)?;
                id.credential = provisioning::fresh_credential().map_err(provision_error)?;
                id.config = b;
                id.enrolled = false;
                id.persist(&store).map_err(provision_error)?;
            }
        } else if let Some(mut id) = existing {
            if id.enrolled || id.config.manager_endpoint != b.manager_endpoint {
                return Err(fail(5, "binding_replacement_requires_pair_replace"));
            }
            id.config = b;
            id.persist(&store).map_err(provision_error)?;
        } else if b.enrollment_token.is_empty() {
            store
                .put(
                    "local-settings.json",
                    &serde_json::to_vec(&settings(&b)).map_err(storage_error)?,
                )
                .map_err(storage_error)?;
        } else {
            store
                .put(
                    "bootstrap.json",
                    &Zeroizing::new(serde_json::to_vec(&b).map_err(storage_error)?),
                )
                .map_err(storage_error)?;
        }
    } else if !resume
        && store
            .read("bootstrap.json")
            .map_err(storage_error)?
            .is_none()
    {
        return Err(fail(2, "protected_input_required"));
    }
    drop(store);
    let rt = tokio::runtime::Runtime::new().map_err(storage_error)?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| fail(9, "pairing_result_uncertain"))?;
    let result = rt.block_on(async {
        tokio::select! {
            r = tokio::time::timeout(remaining, provisioning::pair(path)) =>
                r.map_err(|_| fail(9, "pairing_result_uncertain"))?.map_err(|error| {
                    let step = if matches!(
                        &error,
                        ProvisionError::SunshineCredentialsRejected
                            | ProvisionError::SunshineApiUnavailable
                            | ProvisionError::SunshineVersionUnsupported { .. }
                    ) {
                        "local_sunshine_preflight"
                    } else {
                        "manager_pairing"
                    };
                    provision_error(error).at_step(step)
                }),
            _ = tokio::signal::ctrl_c() => Err(fail(130, "interrupted_resume_required")),
        }
    });
    result.map_err(|mut error| {
        if let Ok(store) = ProtectedState::open_readonly(&path.join("provisioning"))
            && let Ok(Some(saved)) = identity(&store)
        {
            error.transaction_id = Some(saved.binding.installation_id.to_string());
            error.committed |= saved.enrolled;
            if saved.enrolled && error.exit != 130 {
                error.exit = 11;
            }
        }
        error
    })?;
    local_status(path)
}

fn setup(args: &Args, path: PathBuf) -> Result<Value> {
    if path != service().default_config {
        return Err(fail(2, "service_config_mismatch").at_step("configuration"));
    }
    let interactive = !args.has("--non-interactive") && !args.has("--input-stdin");
    let mut steps = Vec::new();
    let service_api = service();
    service_api
        .status(args.timeout)
        .map_err(|error| error.at_step("service_inspection"))?;
    let existing = match read_store(&path).map_err(|error| error.at_step("configuration"))? {
        Some(store) => identity(&store).map_err(|error| error.at_step("configuration"))?,
        None => None,
    };
    if let Some(identity) = existing.as_ref().filter(|identity| identity.enrolled) {
        if args.has("--input-stdin") {
            return Err(
                fail(5, "active_setup_input_requires_pair_replace").at_step("configuration")
            );
        }
        if let Some(server) = args.get("--server") {
            let mut expected = url::Url::parse(server)
                .map_err(|_| fail(2, "invalid_server_origin").at_step("configuration"))?;
            if expected.scheme() != "https"
                || expected.path() != "/"
                || expected.query().is_some()
                || expected.fragment().is_some()
            {
                return Err(fail(2, "invalid_server_origin").at_step("configuration"));
            }
            expected
                .set_scheme("wss")
                .map_err(|_| fail(2, "invalid_server_origin").at_step("configuration"))?;
            expected.set_path(crate::transport::CONNECT_PATH);
            if identity.config.manager_endpoint != expected.as_str() {
                return Err(
                    fail(5, "server_replacement_requires_pair_replace").at_step("configuration")
                );
            }
        }
    }
    record_setup_step(
        &mut steps,
        interactive,
        "configuration",
        "verified",
        json!({"path":path,"service_path_matches":true}),
    );
    let pairing = if existing.as_ref().is_some_and(|id| id.enrolled) {
        json!({"committed":true,"already_active":true})
    } else {
        let service_status = service_api
            .status(args.timeout)
            .map_err(|error| error.at_step("service_quiesce"))?;
        if service_status["state"] == "running" {
            let stopped = service_api
                .change(&setup_service_args(args), "stop", &path)
                .map_err(|error| error.at_step("service_quiesce"))?;
            if stopped["state"] == "running" {
                return Err(fail(11, "service_state_unconfirmed").at_step("service_quiesce"));
            }
        }
        let has_protected_input = args.has("--input-stdin") || args.has("--interactive");
        let resume = if existing.is_some()
            && !has_protected_input
            && args.get("--server").is_none()
            && interactive
        {
            eprintln!("[setup] pairing: pending_request_found");
            ask_yes_no(
                "Resume without submitting a new authorization code?",
                false,
                Instant::now() + args.timeout,
            )
            .map_err(|error| error.at_step("pairing"))?
        } else {
            false
        };
        let pair_words = if resume {
            vec!["pair".into(), "resume".into()]
        } else {
            vec!["pair".into()]
        };
        let interactive = !resume && !args.has("--input-stdin") && !args.has("--non-interactive");
        execute_pair(&setup_args(args, pair_words, interactive), &path).map_err(|mut error| {
            if error.step.is_none() {
                error.step = Some("pairing");
            }
            error
        })?
    };
    let pairing_status =
        local_status(&path).map_err(|error| setup_failure(error, &pairing, "pairing"))?;
    if !pairing_is_active(&pairing_status) {
        return Err(setup_failure(
            fail(11, "pairing_postcondition_unconfirmed"),
            &pairing,
            "pairing",
        ));
    }
    record_setup_step(
        &mut steps,
        interactive,
        "pairing",
        "verified",
        json!({"state":"active","durable_identity":true}),
    );
    let deadline = Instant::now() + args.timeout;
    let enable = setup_autostart(interactive, |label, default| {
        ask_yes_no(label, default, deadline)
    })
    .map_err(|error| setup_failure(error, &pairing, "preferences"))?;
    let registered = service_api
        .verified_status(args.timeout, &path)
        .map_err(|error| setup_failure(error, &pairing, "service_registration"))?;
    record_setup_step(
        &mut steps,
        interactive,
        "service_registration",
        "verified",
        json!({"installed":registered["installed"],"registration_matches":true}),
    );
    let policy_action = if enable { "enable" } else { "disable" };
    let policy_status = service_api
        .change(&setup_service_args(args), policy_action, &path)
        .map_err(|error| setup_failure(error, &pairing, "startup_policy"))?;
    if !startup_policy_matches(&policy_status, enable) {
        return Err(setup_failure(
            fail(11, "startup_policy_unconfirmed"),
            &pairing,
            "startup_policy",
        ));
    }
    record_setup_step(
        &mut steps,
        interactive,
        "startup_policy",
        "verified",
        json!({"requested":if enable {"enabled"} else {"disabled"},"observed":policy_status["startup"]}),
    );
    let service_result = service_api
        .change(&setup_service_args(args), "start", &path)
        .map_err(|error| setup_failure(error, &pairing, "service_runtime"))?;
    if service_result["state"] != "running" {
        return Err(setup_failure(
            fail(11, "service_state_unconfirmed"),
            &pairing,
            "service_runtime",
        ));
    }
    record_setup_step(
        &mut steps,
        interactive,
        "service_runtime",
        "verified",
        json!({"requested":"running","observed":service_result["state"]}),
    );
    let verification = match wait_for_healthy(&path, args.timeout) {
        Ok(status) => {
            record_setup_step(
                &mut steps,
                interactive,
                "connection",
                "verified",
                json!({"health":status["health"]}),
            );
            status
        }
        Err(error) => return Err(setup_failure(error, &pairing, "connection")),
    };
    Ok(json!({
        "setup":"completed",
        "steps":steps,
        "pairing":pairing,
        "service":service_result,
        "verification":verification
    }))
}

fn no_args(args: &Args) -> u8 {
    let status_args = Args {
        words: vec!["status".into()],
        options: args.options.clone(),
        format: args.format.clone(),
        timeout: args.timeout,
    };
    let mut result = execute(&status_args);
    if let Ok(value) = &mut result {
        value["next_steps"] = json!([
            "xscc setup",
            "xscc status",
            "xscc service status",
            "xscc logs"
        ]);
    }
    emit_xscc("status", &args.format, &result)
}

fn validate_product_paths(args: Args) -> Result<Args> {
    for option in ["--file", "--bootstrap"] {
        if let Some(path) = args.get(option) {
            absolute(Path::new(path))?;
        }
    }
    Ok(args)
}

pub fn entry(raw: Vec<String>) -> u8 {
    let parse_format = requested_error_format(&raw);
    #[cfg(windows)]
    let elevation_raw = raw.clone();
    let args = match Args::parse(
        raw,
        &["--bootstrap", "--file", "--server", "--expected-revision"],
        &["--network", "--sunshine"],
    )
    .and_then(validate_product_paths)
    {
        Ok(a) => a,
        Err(e) => return emit_xscc("parse", parse_format, &Err(e)),
    };
    // Help and version are read-only and must remain available without UAC.
    if args.has("--help") {
        println!(
            "xscc: setup; config init|show|edit|validate|diff|apply; pair [status|resume|replace]; credentials update; status; doctor; service status|start|stop|restart|enable|disable; run; version\nGlobal: --format human|json|ndjson --non-interactive --timeout 60s --no-color --config ABSOLUTE_STATE_DIRECTORY (--state is an alias for the same state directory)\nsetup/pair uses --interactive or --input-stdin. Local Sunshine must use an HTTPS loopback IP and valid Sunshine API credentials; its local certificate identity is not checked. setup completes pairing, service startup policy and connection verification. No secret arguments. config edit uses VISUAL or EDITOR and commits through the same revision check as config apply. Services must be stopped for writes."
        );
        return 0;
    }
    if (args.has("--version") || args.words == ["version"]) && args.format == "human" {
        println!(
            "xscc {} (git {}; {})",
            env!("CARGO_PKG_VERSION"),
            env!("XSCC_BUILD_SHA"),
            xscs_protocol::PROTOCOL
        );
        return 0;
    }
    #[cfg(windows)]
    if args.words == ["setup"] {
        let interactive = !args.has("--non-interactive") && !args.has("--input-stdin");
        let installer_session = args.has("--installer-session");
        match prepare_windows_setup_elevation(
            &elevation_raw,
            interactive,
            installer_session,
            args.has("--elevated-setup-child"),
        ) {
            Ok(WindowsSetupElevation::Continue) => {}
            Ok(WindowsSetupElevation::ChildExited(exit)) => {
                if !installer_session {
                    if exit == 0 {
                        println!("Setup completed with administrator privileges.");
                    } else {
                        eprintln!(
                            "Setup failed with exit code {exit}. Run `xscc setup` from an Administrator terminal to keep the error visible."
                        );
                    }
                }
                return exit;
            }
            Err(error) => return emit_xscc("setup", &args.format, &Err(error)),
        }
    }
    if args.words.is_empty() && !args.has("--version") {
        return no_args(&args);
    }
    if args.has("--follow") {
        if args.words != ["logs"] || args.format != "ndjson" {
            return emit_xscc(
                "logs",
                &args.format,
                &Err(fail(2, "follow_requires_logs_ndjson")),
            );
        }
        #[cfg(windows)]
        return follow_log_source("xscc", args, &XsccErrorCatalog, |args| {
            let path = args
                .get("--config")
                .or(args.get("--state"))
                .map(PathBuf::from)
                .unwrap_or_else(default_state);
            windows_runtime_logs(args, &path)
        });
        #[cfg(not(windows))]
        if let Err(error) = args.validate_options(&["--tail", "--since", "--follow"]) {
            return xcsc::cli::emit("xscc", "logs", &args.format, &Err(error), &XsccErrorCatalog);
        }
        #[cfg(not(windows))]
        return follow_logs("xscc", &service(), args, &XsccErrorCatalog);
    }
    if args.has("--watch") {
        if args.words != ["status"] || args.format != "ndjson" {
            return emit_xscc(
                "status",
                &args.format,
                &Err(fail(2, "watch_requires_status_ndjson")),
            );
        }
        let mut single = args;
        single.options.remove("--watch");
        return watch(&single);
    }
    let command = args.words.join(" ");
    let result = execute(&args);
    let exit = emit_xscc(&command, &args.format, &result);
    #[cfg(windows)]
    if args.words == ["setup"]
        && args.has("--installer-session")
        && args.has("--elevated-setup-child")
    {
        pause_installer_setup();
    }
    exit
}
fn execute(args: &Args) -> Result<Value> {
    let words: Vec<_> = args.words.iter().map(String::as_str).collect();
    if args.has("--version") || words == ["version"] {
        args.validate_options(&[])?;
        return Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"commit":env!("XSCC_BUILD_SHA"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"protocol":xscs_protocol::PROTOCOL,"cli_schema_version":1,"config_format":"xscc-bootstrap-v1","state_format":"xscc-identity-journal-v1","ipc_version":1}),
        );
    }
    if args.has("--state") && args.has("--config") {
        return Err(fail(2, "duplicate_state_selection"));
    }
    let path = args
        .get("--config")
        .or(args.get("--state"))
        .map(PathBuf::from)
        .unwrap_or_else(default_state);
    match words.as_slice() {
        #[cfg(windows)]
        ["installer", action]
            if ["prepare-setup", "reset-configuration", "reset-data"].contains(action) =>
        {
            args.validate_options(&[])?;
            if path != default_state() {
                return Err(fail(2, "installer_state_path_mismatch"));
            }
            if *action == "prepare-setup" {
                installer_prepare_setup(&path)
            } else {
                installer_reset(&path, action)
            }
        }
        ["setup"] => {
            args.validate_options(&[
                "--interactive",
                "--input-stdin",
                "--server",
                "--installer-session",
                "--elevated-setup-child",
            ])?;
            setup(args, path)
        }
        ["tasks", "list"] => {
            args.validate_options(&[])?;
            crate::journal::inspect(&path.join("journal"), None).map_err(journal_error)
        }
        ["tasks", "show", id] => {
            args.validate_options(&[])?;
            crate::journal::inspect(&path.join("journal"), Some(id)).map_err(journal_error)
        }

        ["logs"] => {
            #[cfg(windows)]
            args.validate_options(&[
                "--tail",
                "--since",
                "--follow",
                "--instance-id",
                "--event",
                "--request-id",
                "--task-id",
                "--level",
            ])?;
            #[cfg(not(windows))]
            args.validate_options(&["--tail", "--since", "--follow"])?;
            if args.has("--follow") {
                return Err(fail(10, "log_follow_not_available"));
            }
            #[cfg(windows)]
            {
                windows_runtime_logs(args, &path)
            }
            #[cfg(not(windows))]
            {
                service().logs(args)
            }
        }

        ["service", action] => {
            args.validate_options(&["--now"])?;
            if args.has("--now") && !["enable", "disable"].contains(action) {
                return Err(fail(2, "invalid_now_option"));
            }
            if *action == "status" {
                service().status(args.timeout)
            } else {
                service().change(args, action, &path)
            }
        }
        ["status"] | ["pair", "status"] | ["doctor"] => {
            args.validate_options(&["--check", "--watch", "--network", "--sunshine"])?;
            let mut value = local_status(&path)?;
            value["service"] = service()
                .status(args.timeout)
                .unwrap_or(json!({"state":"unknown"}));
            if args.has("--network") {
                if words != ["doctor"] {
                    return Err(fail(2, "probe_requires_doctor"));
                }
                let store = read_store(&path)?.ok_or_else(|| fail(4, "awaiting_configuration"))?;
                let (config, _) = current(&store)?;
                let runtime = tokio::runtime::Runtime::new().map_err(storage_error)?;
                value["network"] = runtime
                    .block_on(async {
                        tokio::time::timeout(args.timeout, provisioning::network_probe(&config))
                            .await
                    })
                    .map_err(|_| fail(9, "network_probe_timeout"))?
                    .map_err(provision_error)?;
            }
            if args.has("--sunshine") {
                if words != ["doctor"] {
                    return Err(fail(2, "probe_requires_doctor"));
                }
                use crate::adapter::Sunshine;
                let store = read_store(&path)?.ok_or_else(|| fail(4, "awaiting_configuration"))?;
                let (b, _) = current(&store)?;
                let mut adapter = b.adapter().map_err(provision_error)?;
                let rt = tokio::runtime::Runtime::new().map_err(storage_error)?;
                let observation = rt
                    .block_on(async { tokio::time::timeout(args.timeout, adapter.read()).await })
                    .map_err(|_| fail(9, "sunshine_probe_timeout"))?;
                let observation = observation.map_err(sunshine_adapter_error)?;
                value["sunshine"] = json!({
                    "endpoint": b.sunshine_endpoint,
                    "tcp": "connected",
                    "tls": "encrypted",
                    "certificate_identity": "not_checked_local_loopback_policy",
                    "api": "available",
                    "sunshine_version": observation.sunshine_version(),
                    "credentials": "accepted",
                    "version_supported": true,
                    "authentication": "sunshine_basic_credentials"
                });
            }
            if args.has("--check") && value["health"] != "healthy" {
                return Err(fail(12, "business_health_unconfirmed"));
            }
            Ok(value)
        }
        ["config", "show"] => {
            args.validate_options(&[])?;
            let store = read_store(&path)?.ok_or_else(|| fail(4, "awaiting_configuration"))?;
            let (b, _) = current(&store)?;
            let mut v = serde_json::to_value(&b).map_err(storage_error)?;
            redact(&mut v);
            Ok(json!({"config":v,"stored_revision":settings_revision(&b)?}))
        }
        ["config", "edit"] => {
            args.validate_options(&[])?;
            let store = read_store(&path)?.ok_or_else(|| fail(4, "awaiting_configuration"))?;
            let (current_config, _) = current(&store)?;
            let before = settings_revision(&current_config)?;
            let edited = edit_json(
                &serde_json::to_value(settings(&current_config)).map_err(storage_error)?,
            )?;
            let candidate: Settings = serde_json::from_value(edited).map_err(input_error)?;
            validate_settings(&candidate)?;
            drop(store);
            let _guard = MaintenanceGuard::acquire(&path).map_err(storage_error)?;
            let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
            let (mut config, mut identity) = current(&store)?;
            if settings_revision(&config)? != before {
                return Err(fail(5, "revision_conflict"));
            }
            config.sunshine_endpoint = candidate.sunshine_endpoint;
            let after = settings_revision(&config)?;
            if let Some(identity) = identity.as_mut() {
                identity.config = config;
                identity.persist(&store).map_err(provision_error)?;
            } else if config.enrollment_token.is_empty() {
                store
                    .put(
                        "local-settings.json",
                        &serde_json::to_vec(&settings(&config)).map_err(storage_error)?,
                    )
                    .map_err(storage_error)?;
            } else {
                store
                    .put(
                        "bootstrap.json",
                        &Zeroizing::new(serde_json::to_vec(&config).map_err(storage_error)?),
                    )
                    .map_err(storage_error)?;
            }
            Ok(
                json!({"committed":true,"previous_revision":before,"stored_revision":after,"effective_revision":null,"restart_required":true}),
            )
        }
        ["config", action] if ["validate", "diff", "apply"].contains(action) => {
            args.validate_options(&["--input-stdin", "--file", "--expected-revision"])?;
            let candidate: Settings = document(args)?;
            validate_settings(&candidate)?;
            if *action == "validate" {
                return Ok(
                    json!({"valid":true,"candidate_revision":revision(&serde_json::to_vec(&candidate).map_err(input_error)?)}),
                );
            }
            let _guard = if *action == "apply" {
                Some(MaintenanceGuard::acquire(&path).map_err(storage_error)?)
            } else {
                None
            };
            let store = if *action == "apply" {
                ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?
            } else {
                read_store(&path)?.ok_or_else(|| fail(4, "awaiting_configuration"))?
            };
            let (mut b, mut id) = current(&store)?;
            let before = settings_revision(&b)?;
            let old = serde_json::to_value(settings(&b)).map_err(storage_error)?;
            b.sunshine_endpoint = candidate.sunshine_endpoint;
            let after = settings_revision(&b)?;
            if *action == "apply" {
                if args.require("--expected-revision")? != before {
                    return Err(fail(5, "revision_conflict"));
                }
                if let Some(id) = id.as_mut() {
                    id.config = b;
                    id.persist(&store).map_err(provision_error)?;
                } else if b.enrollment_token.is_empty() {
                    store
                        .put(
                            "local-settings.json",
                            &serde_json::to_vec(&settings(&b)).map_err(storage_error)?,
                        )
                        .map_err(storage_error)?;
                } else {
                    store
                        .put(
                            "bootstrap.json",
                            &Zeroizing::new(serde_json::to_vec(&b).map_err(storage_error)?),
                        )
                        .map_err(storage_error)?;
                }
                Ok(
                    json!({"committed":true,"stored_revision":after,"effective_revision":null,"restart_required":true}),
                )
            } else {
                Ok(
                    json!({"valid":true,"stored_revision":before,"candidate_revision":after,"before":old,"after":settings(&b)}),
                )
            }
        }
        ["credentials", "update"] => {
            args.validate_options(&["--interactive", "--input-stdin"])?;
            let c: Credentials = if args.has("--interactive") {
                let deadline = Instant::now() + args.timeout;
                Credentials {
                    sunshine_username: Zeroizing::new(prompt_text(
                        "Sunshine username",
                        MAX_SUNSHINE_USERNAME_BYTES,
                        deadline,
                    )?),
                    sunshine_password: prompt_secret(
                        "Sunshine password",
                        MAX_SUNSHINE_PASSWORD_BYTES,
                        deadline,
                    )?,
                }
            } else {
                document(args)?
            };
            let _guard = MaintenanceGuard::acquire(&path).map_err(storage_error)?;
            let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
            let mut id = identity(&store)?.ok_or_else(|| fail(4, "awaiting_pairing"))?;
            id.config.sunshine_username = c.sunshine_username;
            id.config.sunshine_password = c.sunshine_password;
            id.config.adapter().map_err(provision_error)?;
            id.persist(&store).map_err(provision_error)?;
            Ok(json!({"committed":true,"binding":id.binding,"restart_required":true}))
        }
        ["config", "init"] => {
            args.validate_options(&["--interactive"])?;
            let candidate = Settings {
                sunshine_endpoint: if args.has("--interactive") {
                    prompt_text(
                        "Local Sunshine HTTPS URL",
                        MAX_URL_BYTES,
                        Instant::now() + args.timeout,
                    )?
                } else {
                    "https://127.0.0.1:47990/".into()
                },
            };
            validate_settings(&candidate)?;
            let _guard = MaintenanceGuard::acquire(&path).map_err(storage_error)?;
            let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
            if store
                .read("local-settings.json")
                .map_err(storage_error)?
                .is_some()
                || store
                    .read("bootstrap.json")
                    .map_err(storage_error)?
                    .is_some()
                || identity(&store)?.is_some()
            {
                return Err(fail(5, "configuration_already_exists"));
            }
            store
                .put(
                    "local-settings.json",
                    &serde_json::to_vec(&candidate).map_err(storage_error)?,
                )
                .map_err(storage_error)?;
            Ok(
                json!({"initialized":true,"pairing":"awaiting_input","next_step":"pair --interactive or pair --input-stdin"}),
            )
        }
        ["init"] => {
            args.validate_options(&["--bootstrap"])?;
            let _guard = MaintenanceGuard::acquire(&path).map_err(storage_error)?;
            let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
            store
                .import_bootstrap(Path::new(args.require("--bootstrap")?))
                .map_err(storage_error)?;
            Ok(json!({"imported":true,"next_step":"pair explicitly before starting service"}))
        }
        ["pair"] | ["pair", "resume"] | ["pair", "replace"] => execute_pair(args, &path),
        ["run"] => {
            args.validate_options(&[])?;
            run(&path)?;
            Ok(json!({"stopped":true}))
        }
        _ => Err(fail(2, "unknown_command")),
    }
}

#[cfg(any(windows, test))]
fn installer_prepare_setup(path: &Path) -> Result<Value> {
    // The installer may repair account compatibility, but it must never make
    // an incompatible durable execution journal disappear. Validate important
    // data before taking the provisioning lock or archiving an account record.
    ensure_execution_journal_compatible(path)?;
    let provisioning = path.join("provisioning");
    match std::fs::symlink_metadata(&provisioning) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"prepared":true,"archived":[]}));
        }
        Err(error) => return Err(storage_error(error)),
        Ok(_) => {}
    }

    let _guard = MaintenanceGuard::acquire(path)
        .map_err(|error| storage_error(error).at_step("installer_maintenance_lock"))?;
    let store = ProtectedState::open(&provisioning)
        .map_err(|error| storage_error(error).at_step("installer_provisioning_open"))?;
    let mut archived = Vec::new();
    for (name, artifact) in [
        ("identity.json", "identity"),
        ("bootstrap.json", "bootstrap"),
    ] {
        let Some(bytes) = store
            .read(name)
            .map_err(|error| storage_error(error).at_step("installer_account_read"))?
        else {
            continue;
        };
        let compatibility = if name == "identity.json" {
            provisioning::decode_identity(&Zeroizing::new(bytes)).map(|_| ())
        } else {
            provisioning::decode_bootstrap(&Zeroizing::new(bytes)).map(|_| ())
        };
        match compatibility {
            Ok(()) => {}
            Err(ProvisionError::StateDocumentCorrupt { .. }) => {
                let archive_name = format!(
                    "{artifact}.incompatible-{}.json",
                    uuid::Uuid::new_v4().simple()
                );
                store
                    .archive(name, &archive_name)
                    .map_err(|error| storage_error(error).at_step("installer_account_archive"))?;
                archived.push(archive_name);
            }
            Err(error) => {
                return Err(provision_error(error).at_step("installer_account_validation"));
            }
        }
    }
    Ok(json!({"prepared":true,"archived":archived,"important_data":"preserved"}))
}

#[cfg(windows)]
fn installer_reset(path: &Path, action: &str) -> Result<Value> {
    let (target, category) = match action {
        "reset-configuration" => (path.join("provisioning"), "configuration"),
        "reset-data" => (path.join("journal"), "execution data"),
        _ => return Err(fail(2, "invalid_installer_reset")),
    };
    remove_installer_tree(&target).map_err(|error| {
        fail(8, "unsafe_or_corrupt_state")
            .with_detail(format!("artifact={category};preserved=true;error={error}"))
    })?;
    Ok(json!({"removed":category,"path":target}))
}

#[cfg(any(windows, test))]
fn remove_installer_tree(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "installer target is not a regular non-link directory",
        ));
    }
    let mut pending = vec![path.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            entries = entries
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("installer state entry count overflow"))?;
            if entries > 100_000 {
                return Err(std::io::Error::other(
                    "installer state exceeds the 100000-entry safety limit",
                ));
            }
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "installer state contains a symbolic link or junction",
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if !metadata.is_file() {
                return Err(std::io::Error::other(
                    "installer state contains an unsupported file type",
                ));
            }
        }
    }
    std::fs::remove_dir_all(path)
}
pub fn run(path: &Path) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().map_err(storage_error)?;
    rt.block_on(async {
        let (tx,rx)=tokio::sync::watch::channel(false);
        let stop=async {
            #[cfg(unix)] {let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(storage_error)?;tokio::select!{_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}}
            #[cfg(windows)] tokio::signal::ctrl_c().await.map_err(storage_error)?;
            let _=tx.send(true);Ok::<(),Failure>(())
        };
        let runtime=provisioning::run(path,rx);tokio::pin!(runtime);
        tokio::select!{r=&mut runtime=>r.map_err(provision_error),s=stop=>{s?;tokio::time::timeout(std::time::Duration::from_secs(30),runtime).await.map_err(|_|fail(9,"shutdown_result_uncertain"))?.map_err(provision_error)}}
    })
}

fn watch(args: &Args) -> u8 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return 8,
    };
    rt.block_on(async {let deadline=tokio::time::Instant::now()+args.timeout;loop {
        let result=execute(args);let code=emit_xscc("status","ndjson",&result);if code!=0 {return code;}
        tokio::select!{_=tokio::signal::ctrl_c()=>return 130,_=tokio::time::sleep_until(deadline)=>return 0,_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
    }})
}

fn validate_settings(settings: &Settings) -> Result<()> {
    crate::adapter::LocalSunshine::new(
        &settings.sunshine_endpoint,
        "validation",
        Zeroizing::new("validation".into()),
    )
    .map_err(input_error)?;
    Ok(())
}

#[cfg(windows)]
fn windows_runtime_logs(
    args: &xcsc::cli::Args,
    path: &std::path::Path,
) -> xcsc::cli::Result<serde_json::Value> {
    use xcsc::cli::{fail, storage_error};
    use xcsc::fs_safety::{EntryName, Error, PrivateDirectory};
    let directory = PrivateDirectory::open_existing(path.join("logs")).map_err(storage_error)?;
    let level = args
        .get("--level")
        .map(|value| {
            serde_json::from_value::<xcsc::log::Level>(serde_json::json!(
                value.to_ascii_uppercase()
            ))
        })
        .transpose()
        .map_err(|_| fail(2, "invalid_log_level"))?;
    xcsc::cli::query_rotating_logs(
        args,
        "xscc",
        |name| match directory.read_private_bounded(
            &EntryName::new(name).map_err(storage_error)?,
            8 * 1024 * 1024,
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(fail(8, "unsafe_or_unreadable_runtime_log")),
        },
        |bytes| {
            xcsc::log::query(
                std::io::Cursor::new(bytes),
                xcsc::log::LogFilter {
                    since: args.get("--since"),
                    instance_id: args.get("--instance-id"),
                    event: args.get("--event"),
                    request_id: args.get("--request-id"),
                    task_id: args.get("--task-id"),
                    minimum_level: level,
                    ..Default::default()
                },
                xcsc::log::QueryLimits {
                    max_input_bytes: 1024 * 1024,
                    max_records: 16384,
                },
            )
            .map_err(|_| fail(8, "log_integrity_or_filter_failed"))?
            .into_iter()
            .map(|record| serde_json::to_value(record).map_err(storage_error))
            .collect()
        },
    )
}

#[cfg(test)]
#[path = "cli/tests.rs"]
mod setup_tests;
