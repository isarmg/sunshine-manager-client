use crate::{
    provisioning::{self, Bootstrap, Identity, ProvisionError},
    storage::{MaintenanceGuard, ProtectedState},
};
use rand::RngCore;
use sarmg_client_cli::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

fn default_state() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"C:\ProgramData\SunshineClient"
    } else if cfg!(target_os = "macos") {
        "/Library/Application Support/sunshine-client"
    } else {
        "/var/lib/sunshine-client"
    })
}
fn service() -> Service {
    Service {
        #[cfg(not(target_os = "macos"))]
        name: if cfg!(windows) {
            "SunshineClient"
        } else {
            "sunshine-client.service"
        },
        label: "org.sarmg.sunshine-client",
        default_config: default_state(),
        binary: "sunshine-client",
        log_path: "/var/log/sunshine-client.log",
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
    }
}
fn provision_error(e: ProvisionError) -> Failure {
    match e {
        ProvisionError::Configuration => fail(2, "invalid_configuration"),
        ProvisionError::Rejected => fail(7, "credential_rejected"),
        ProvisionError::Unavailable | ProvisionError::RateLimited => fail(6, "server_unavailable"),
        ProvisionError::Unsupported => fail(10, "unsupported_protocol_or_platform"),
        ProvisionError::SunshineCertificateUntrusted => fail(6, "sunshine_certificate_untrusted"),
        ProvisionError::SunshineCertificateMismatch => fail(7, "sunshine_certificate_mismatch"),
        ProvisionError::SunshineCredentialsRejected => fail(7, "sunshine_credentials_rejected"),
        ProvisionError::SunshineApiUnavailable => fail(6, "sunshine_api_unavailable"),
        ProvisionError::SunshineVersionUnsupported => fail(10, "sunshine_version_unsupported"),
        ProvisionError::Unpaired => fail(4, "awaiting_pairing"),
        ProvisionError::Transport(error) => match error {
            crate::transport::TransportError::Configuration => fail(2, "invalid_configuration"),
            crate::transport::TransportError::Revoked => fail(7, "credential_rejected"),
            crate::transport::TransportError::Disconnected => fail(6, "server_unavailable"),
            crate::transport::TransportError::Protocol => {
                fail(10, "unsupported_protocol_or_platform")
            }
        },
        ProvisionError::Storage(error) => storage_error(error),
        ProvisionError::Journal => fail(8, "protected_state_failure"),
    }
}

fn sunshine_adapter_error(error: crate::adapter::AdapterError) -> Failure {
    match error {
        crate::adapter::AdapterError::CertificateUntrusted => {
            fail(6, "sunshine_certificate_untrusted")
                .with_detail("tcp=connected tls=failed api=not_attempted credentials=not_attempted version=not_checked")
        }
        crate::adapter::AdapterError::CertificateMismatch => {
            fail(7, "sunshine_certificate_mismatch")
                .with_detail("tcp=connected tls=certificate_mismatch api=not_attempted credentials=not_attempted version=not_checked")
        }
        crate::adapter::AdapterError::CredentialsRejected => {
            fail(7, "sunshine_credentials_rejected")
                .with_detail("tcp=connected tls=verified api=available credentials=rejected version=not_checked")
        }
        crate::adapter::AdapterError::ApiUnavailable => fail(6, "sunshine_api_unavailable")
            .with_detail("tcp=unknown tls=unknown api=unavailable credentials=unknown version=not_checked"),
        crate::adapter::AdapterError::UnsupportedVersion => {
            fail(10, "sunshine_version_unsupported")
                .with_detail("tcp=connected tls=verified api=available credentials=accepted version=unsupported")
        }
        crate::adapter::AdapterError::UnsafeConfiguration
        | crate::adapter::AdapterError::InvalidLocalEndpoint => {
            fail(2, "invalid_sunshine_configuration")
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairInput {
    server: String,
    authorization_code: Zeroizing<String>,
    sunshine_endpoint: String,
    #[serde(default)]
    sunshine_certificate: Option<String>,
    #[serde(default)]
    sunshine_certificate_path: Option<PathBuf>,
    sunshine_username: Zeroizing<String>,
    sunshine_password: Zeroizing<String>,
    #[serde(default)]
    restart_allowed: bool,
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
        url.set_path("/sunshine-client/v1/connect");
        if self.sunshine_certificate.is_some() && self.sunshine_certificate_path.is_some() {
            return Err(fail(2, "duplicate_sunshine_certificate_source"));
        }
        let sunshine_certificate = match (self.sunshine_certificate, self.sunshine_certificate_path)
        {
            (Some(pem), None) => Some(pem),
            (None, Some(path)) => Some(read_sunshine_certificate(&path)?),
            (None, None) => None,
            (Some(_), Some(_)) => unreachable!(),
        };
        let b = Bootstrap {
            manager_endpoint: url.to_string(),
            enrollment_token: self.authorization_code,
            sunshine_endpoint: self.sunshine_endpoint,
            sunshine_certificate,
            sunshine_username: self.sunshine_username,
            sunshine_password: self.sunshine_password,
            restart_allowed: self.restart_allowed,
        };
        b.validate().map_err(provision_error)?;
        Ok(b)
    }
}

fn read_sunshine_certificate(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        return Err(fail(2, "sunshine_certificate_path_must_be_absolute"));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(certificate_io_error)?;
    if !safe_certificate_metadata(&metadata) || metadata.len() > 64 * 1024 {
        return Err(fail(2, "invalid_sunshine_certificate_file"));
    }
    let bytes = std::fs::read(path).map_err(certificate_io_error)?;
    if bytes.len() > 64 * 1024 {
        return Err(fail(2, "invalid_sunshine_certificate_file"));
    }
    crate::adapter::certificate_sha256_fingerprint(&bytes)
        .map_err(|_| fail(2, "invalid_sunshine_certificate_file"))?;
    String::from_utf8(bytes).map_err(|_| fail(2, "invalid_sunshine_certificate_file"))
}

fn certificate_io_error(error: std::io::Error) -> Failure {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => fail(3, "sunshine_certificate_unreadable"),
        _ => fail(2, "invalid_sunshine_certificate_file"),
    }
}

fn safe_certificate_metadata(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn sunshine_service_executable() -> Option<PathBuf> {
    use windows_service::{
        service::ServiceAccess,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager
        .open_service("SunshineService", ServiceAccess::QUERY_CONFIG)
        .ok()?;
    service
        .query_config()
        .ok()
        .map(|config| config.executable_path)
}

#[cfg(not(windows))]
fn sunshine_service_executable() -> Option<PathBuf> {
    None
}

fn extend_windows_certificate_candidates(
    candidates: &mut Vec<PathBuf>,
    program_files: Option<PathBuf>,
    program_w6432: Option<PathBuf>,
    service_executable: Option<PathBuf>,
) {
    for directory in [program_files, program_w6432].into_iter().flatten() {
        candidates.push(directory.join("Sunshine/config/credentials/cacert.pem"));
    }
    if let Some(executable) = service_executable
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("config/credentials/cacert.pem"));
        candidates.push(directory.join("config/cacert.pem"));
    }
}

fn sunshine_certificate_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(directory) = std::env::var_os("SUNSHINE_CONFIG_DIR") {
        candidates.push(PathBuf::from(directory).join("cacert.pem"));
    }
    if let Some(directory) = std::env::var_os("XDG_CONFIG_HOME") {
        candidates.push(PathBuf::from(directory).join("sunshine/cacert.pem"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".config/sunshine/cacert.pem"));
    }
    if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
        candidates.push(PathBuf::from(program_data).join("Sunshine/config/credentials/cacert.pem"));
    }
    extend_windows_certificate_candidates(
        &mut candidates,
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramW6432").map(PathBuf::from),
        sunshine_service_executable(),
    );
    candidates.extend([
        PathBuf::from("/etc/sunshine/cacert.pem"),
        PathBuf::from("/var/lib/sunshine/.config/sunshine/cacert.pem"),
    ]);
    candidates.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    candidates.dedup_by(|left, right| {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    });
    candidates.retain(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| {
            safe_certificate_metadata(&metadata)
                && metadata.len() <= 64 * 1024
                && std::fs::read(path)
                    .is_ok_and(|pem| crate::adapter::certificate_sha256_fingerprint(&pem).is_ok())
        })
    });
    candidates
}

fn prompt_sunshine_certificate() -> Result<Option<PathBuf>> {
    let candidates = sunshine_certificate_candidates();
    if !candidates.is_empty() {
        eprintln!("Discovered local Sunshine public certificates:");
        for (index, path) in candidates.iter().enumerate() {
            let fingerprint = std::fs::read(path)
                .ok()
                .and_then(|pem| crate::adapter::certificate_sha256_fingerprint(&pem).ok())
                .unwrap_or_else(|| "unavailable".into());
            eprintln!(
                "  {}. {} (SHA-256 {})",
                index + 1,
                path.display(),
                fingerprint
            );
        }
    }
    let answer = prompt(
        if candidates.is_empty() {
            "Sunshine cacert.pem absolute path (or type 'system' explicitly)"
        } else {
            "Select certificate number/path [1], or type 'system' explicitly"
        },
        false,
    )?;
    if answer == "system" {
        return Ok(None);
    }
    if answer.is_empty() && !candidates.is_empty() {
        return Ok(Some(candidates[0].clone()));
    }
    if let Ok(index) = answer.parse::<usize>()
        && (1..=candidates.len()).contains(&index)
    {
        return Ok(Some(candidates[index - 1].clone()));
    }
    if answer.is_empty() {
        return Err(fail(2, "sunshine_certificate_selection_required"));
    }
    Ok(Some(PathBuf::from(answer)))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    sunshine_endpoint: String,
    #[serde(default)]
    restart_allowed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    sunshine_username: Zeroizing<String>,
    sunshine_password: Zeroizing<String>,
    #[serde(default)]
    sunshine_certificate: Option<String>,
    #[serde(default)]
    sunshine_certificate_path: Option<PathBuf>,
    #[serde(default)]
    use_system_trust: bool,
}

fn credential_certificate_update(credentials: &Credentials) -> Result<Option<Option<String>>> {
    if credentials.use_system_trust
        && (credentials.sunshine_certificate.is_some()
            || credentials.sunshine_certificate_path.is_some())
    {
        return Err(fail(2, "conflicting_sunshine_trust_update"));
    }
    if credentials.sunshine_certificate.is_some() && credentials.sunshine_certificate_path.is_some()
    {
        return Err(fail(2, "duplicate_sunshine_certificate_source"));
    }
    if credentials.use_system_trust {
        return Ok(Some(None));
    }
    match (
        credentials.sunshine_certificate.as_ref(),
        credentials.sunshine_certificate_path.as_deref(),
    ) {
        (Some(pem), None) => Ok(Some(Some(pem.clone()))),
        (None, Some(path)) => Ok(Some(Some(read_sunshine_certificate(path)?))),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => unreachable!(),
    }
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
        .map(|b| serde_json::from_slice(&Zeroizing::new(b)).map_err(storage_error))
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
                sunshine_certificate: None,
                sunshine_username: Zeroizing::new(String::new()),
                sunshine_password: Zeroizing::new(String::new()),
                restart_allowed: settings.restart_allowed,
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
        Ok((serde_json::from_slice(&bytes).map_err(storage_error)?, None))
    }
}
fn settings(b: &Bootstrap) -> Settings {
    Settings {
        sunshine_endpoint: b.sunshine_endpoint.clone(),
        restart_allowed: b.restart_allowed,
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

fn ask_yes_no(label: &str, default: bool) -> Result<bool> {
    let value = prompt(
        &format!("{label} [{}]", if default { "yes" } else { "no" }),
        false,
    )?;
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "y" | "yes" | "true" | "1" => Ok(true),
        "n" | "no" | "false" | "0" => Ok(false),
        _ => Err(fail(2, "invalid_confirmation")),
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
        matches!(observed, "automatic" | "enabled" | "enabled-runtime")
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
        if std::time::Instant::now() >= deadline {
            return Err(fail(9, "connection_unconfirmed"));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn execute_pair(args: &Args, path: &Path) -> Result<Value> {
    let words: Vec<_> = args.words.iter().map(String::as_str).collect();
    args.validate_options(&["--interactive", "--input-stdin", "--server"])?;
    let resume = words == ["pair", "resume"];
    let replace = words == ["pair", "replace"];
    if resume && (args.has("--interactive") || args.has("--input-stdin") || args.has("--server")) {
        return Err(fail(2, "resume_uses_existing_transaction"));
    }
    let incoming = if args.has("--interactive") {
        Some(
            PairInput {
                server: if let Some(s) = args.get("--server") {
                    s.into()
                } else {
                    prompt("Server HTTPS origin", false)?
                },
                authorization_code: Zeroizing::new(prompt("Authorization code", true)?),
                sunshine_endpoint: prompt("Local Sunshine HTTPS URL", false)?,
                sunshine_certificate: None,
                sunshine_certificate_path: prompt_sunshine_certificate()?,
                sunshine_username: Zeroizing::new(prompt("Sunshine username", false)?),
                sunshine_password: Zeroizing::new(prompt("Sunshine password", true)?),
                restart_allowed: prompt("Allow controlled restart? [no]", false)? == "yes",
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
    let existing = identity(&store)?;
    if resume && existing.is_none() {
        return Err(fail(4, "no_pairing_transaction"));
    }
    if let Some(b) = incoming {
        if replace {
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
            let mut random = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut random);
            id.credential =
                Zeroizing::new(random.iter().map(|byte| format!("{byte:02x}")).collect());
            id.config = b;
            id.enrolled = false;
            id.persist(&store).map_err(provision_error)?;
        } else if let Some(id) = existing {
            if id.enrolled || id.config != b {
                return Err(fail(5, "binding_replacement_requires_pair_replace"));
            }
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
    let result = rt.block_on(async {
        tokio::select! {
            r = tokio::time::timeout(args.timeout, provisioning::pair(path)) =>
                r.map_err(|_| fail(9, "pairing_result_uncertain"))?.map_err(provision_error),
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
    let initial_service = service_api
        .status(args.timeout)
        .map_err(|error| error.at_step("service_inspection"))?;
    let (default_enable, default_start) = setup_service_intent(&initial_service);
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
            expected.set_path("/sunshine-client/v1/connect");
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
        let resume = existing.is_some()
            && !has_protected_input
            && args.get("--server").is_none()
            && !args.has("--non-interactive");
        let pair_words = if resume {
            vec!["pair".into(), "resume".into()]
        } else {
            vec!["pair".into()]
        };
        let interactive = !resume && !args.has("--input-stdin") && !args.has("--non-interactive");
        execute_pair(&setup_args(args, pair_words, interactive), &path)
            .map_err(|error| error.at_step("pairing"))?
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
    let (enable, start, verify) = if interactive {
        (
            ask_yes_no("Enable service at system startup?", default_enable)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
            ask_yes_no("Run the service now?", default_start)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
            ask_yes_no("Verify the connection now?", default_start)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
        )
    } else {
        (default_enable, default_start, default_start)
    };
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
    let service_result = if start {
        let status = service_api
            .change(&setup_service_args(args), "start", &path)
            .map_err(|error| setup_failure(error, &pairing, "service_runtime"))?;
        if status["state"] != "running" {
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
            json!({"requested":"running","observed":status["state"]}),
        );
        status
    } else {
        let current = service_api
            .verified_status(args.timeout, &path)
            .map_err(|error| setup_failure(error, &pairing, "service_runtime"))?;
        let status = if current["state"] == "running" {
            service_api
                .change(&setup_service_args(args), "stop", &path)
                .map_err(|error| setup_failure(error, &pairing, "service_runtime"))?
        } else {
            current
        };
        record_setup_step(
            &mut steps,
            interactive,
            "service_runtime",
            "skipped",
            json!({"reason":"not_requested","observed":status["state"]}),
        );
        status
    };
    let verification = if verify {
        if service_result["state"] != "running" {
            return Err(setup_failure(
                fail(4, "verification_requires_running_service"),
                &pairing,
                "connection",
            ));
        }
        match wait_for_healthy(&path, args.timeout) {
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
        }
    } else {
        record_setup_step(
            &mut steps,
            interactive,
            "connection",
            "skipped",
            json!({"reason":"not_requested"}),
        );
        json!({"state":"skipped","reason":"not_requested"})
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
            "sunshine-client setup",
            "sunshine-client status",
            "sunshine-client service status",
            "sunshine-client logs"
        ]);
    }
    emit("sunshine-client", "status", &args.format, &result)
}

pub fn entry(raw: Vec<String>) -> u8 {
    let parse_format = requested_error_format(&raw);
    #[cfg(windows)]
    let elevation_raw = raw.clone();
    let args = match Args::parse(
        raw,
        &["--bootstrap", "--file", "--server", "--expected-revision"],
        &["--network", "--sunshine"],
    ) {
        Ok(a) => a,
        Err(e) => return emit("sunshine-client", "parse", parse_format, &Err(e)),
    };
    // Help and version are read-only and must remain available without UAC.
    if args.has("--help") {
        println!(
            "sunshine-client: setup; config init|show|edit|validate|diff|apply; pair [status|resume|replace]; credentials update; status; doctor; service status|start|stop|restart|enable|disable; run; version\nGlobal: --format human|json|ndjson --non-interactive --timeout 60s --no-color --config ABSOLUTE_STATE_DIRECTORY (--state compatibility alias)\nsetup/pair uses --interactive or --input-stdin. Sunshine's built-in certificate is accepted only when sunshine_certificate_path or sunshine_certificate is supplied through protected input. setup completes pairing, service startup policy and connection verification. No secret arguments. config edit uses VISUAL or EDITOR and commits through the same revision check as config apply. Services must be stopped for writes."
        );
        return 0;
    }
    if (args.has("--version") || args.words == ["version"]) && args.format == "human" {
        println!(
            "sunshine-client {} (git {}; {})",
            env!("CARGO_PKG_VERSION"),
            env!("SUNSHINE_CLIENT_BUILD_SHA"),
            sunshine_client_protocol::PROTOCOL
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
                            "Setup failed with exit code {exit}. Run `sunshine-client setup` from an Administrator terminal to keep the error visible."
                        );
                    }
                }
                return exit;
            }
            Err(error) => return emit("sunshine-client", "setup", &args.format, &Err(error)),
        }
    }
    if args.words.is_empty() && !args.has("--version") {
        return no_args(&args);
    }
    if args.has("--follow") {
        if args.words != ["logs"] || args.format != "ndjson" {
            return emit(
                "sunshine-client",
                "logs",
                &args.format,
                &Err(fail(2, "follow_requires_logs_ndjson")),
            );
        }
        return follow_logs("sunshine-client", &service(), args);
    }
    if args.has("--watch") {
        if args.words != ["status"] || args.format != "ndjson" {
            return emit(
                "sunshine-client",
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
    let exit = emit("sunshine-client", &command, &args.format, &result);
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
            json!({"version":env!("CARGO_PKG_VERSION"),"commit":env!("SUNSHINE_CLIENT_BUILD_SHA"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"protocol":sunshine_client_protocol::PROTOCOL,"cli_schema_version":1,"config_format":"sunshine-bootstrap-v1","state_format":"sunshine-identity-journal-v1","ipc_version":1}),
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
            crate::journal::inspect(&path.join("journal"), None).map_err(storage_error)
        }
        ["tasks", "show", id] => {
            args.validate_options(&[])?;
            crate::journal::inspect(&path.join("journal"), Some(id)).map_err(storage_error)
        }

        ["logs"] => {
            args.validate_options(&["--tail", "--since", "--follow"])?;
            if args.has("--follow") {
                return Err(fail(10, "log_follow_not_available"));
            }
            service().logs(args)
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
                    .block_on(provisioning::network_probe(&config))
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
                    "certificate_path": Value::Null,
                    "certificate_sha256": b.sunshine_certificate.as_deref()
                        .and_then(|pem| crate::adapter::certificate_sha256_fingerprint(pem.as_bytes()).ok()),
                    "certificate_mode": if b.sunshine_certificate.is_some() {"exact_pin"} else {"system_trust"},
                    "tcp": "connected",
                    "tls": "verified",
                    "api": "available",
                    "sunshine_version": observation.sunshine_version(),
                    "credentials": "accepted",
                    "version_supported": true,
                    "trust_context": if b.sunshine_certificate.is_some() {"exact_certificate_pin"} else {"current_cli_account"}
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
            config.restart_allowed = candidate.restart_allowed;
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
            b.restart_allowed = candidate.restart_allowed;
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
                let trust = prompt(
                    "New Sunshine cacert.pem path (blank keeps current, 'system' uses system trust)",
                    false,
                )?;
                let (sunshine_certificate_path, use_system_trust) = match trust.as_str() {
                    "" => (None, false),
                    "system" => (None, true),
                    value => (Some(PathBuf::from(value)), false),
                };
                Credentials {
                    sunshine_username: Zeroizing::new(prompt("Sunshine username", false)?),
                    sunshine_password: Zeroizing::new(prompt("Sunshine password", true)?),
                    sunshine_certificate: None,
                    sunshine_certificate_path,
                    use_system_trust,
                }
            } else {
                document(args)?
            };
            let _guard = MaintenanceGuard::acquire(&path).map_err(storage_error)?;
            let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
            let mut id = identity(&store)?.ok_or_else(|| fail(4, "awaiting_pairing"))?;
            let certificate = credential_certificate_update(&c)?;
            id.config.sunshine_username = c.sunshine_username;
            id.config.sunshine_password = c.sunshine_password;
            if let Some(certificate) = certificate {
                id.config.sunshine_certificate = certificate;
            }
            id.config.adapter().map_err(provision_error)?;
            id.persist(&store).map_err(provision_error)?;
            Ok(json!({"committed":true,"binding":id.binding,"restart_required":true}))
        }
        ["config", "init"] => {
            args.validate_options(&["--interactive"])?;
            let candidate = Settings {
                sunshine_endpoint: if args.has("--interactive") {
                    prompt("Local Sunshine HTTPS URL", false)?
                } else {
                    "https://127.0.0.1:47990/".into()
                },
                restart_allowed: false,
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
        let result=execute(args);let code=emit("sunshine-client","status","ndjson",&result);if code!=0 {return code;}
        tokio::select!{_=tokio::signal::ctrl_c()=>return 130,_=tokio::time::sleep_until(deadline)=>return 0,_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
    }})
}

fn validate_settings(settings: &Settings) -> Result<()> {
    crate::adapter::LocalSunshine::new(
        &settings.sunshine_endpoint,
        "validation",
        Zeroizing::new("validation".into()),
        &[],
    )
    .map_err(input_error)?;
    Ok(())
}

#[cfg(test)]
mod setup_tests {
    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            sunshine_username: Zeroizing::new("user".into()),
            sunshine_password: Zeroizing::new("password".into()),
            sunshine_certificate: None,
            sunshine_certificate_path: None,
            use_system_trust: false,
        }
    }

    #[test]
    fn credential_update_distinguishes_keep_pin_and_system_trust() {
        assert_eq!(credential_certificate_update(&credentials()).unwrap(), None);
        let mut pinned = credentials();
        pinned.sunshine_certificate = Some("certificate".into());
        assert_eq!(
            credential_certificate_update(&pinned).unwrap(),
            Some(Some("certificate".into()))
        );
        let mut system = credentials();
        system.use_system_trust = true;
        assert_eq!(credential_certificate_update(&system).unwrap(), Some(None));
        system.sunshine_certificate = Some("certificate".into());
        assert_eq!(
            credential_certificate_update(&system).unwrap_err().code,
            "conflicting_sunshine_trust_update"
        );
    }

    #[test]
    fn startup_policy_requires_a_verified_platform_state() {
        assert!(startup_policy_matches(
            &json!({"startup":"automatic"}),
            true
        ));
        assert!(startup_policy_matches(&json!({"startup":"enabled"}), true));
        assert!(startup_policy_matches(&json!({"startup":"manual"}), false));
        assert!(startup_policy_matches(
            &json!({"startup":"disabled"}),
            false
        ));
        assert!(!startup_policy_matches(&json!({"startup":"unknown"}), true));
        assert!(!startup_policy_matches(&json!({"startup":"manual"}), true));
    }

    #[test]
    fn setup_failures_preserve_pairing_and_identify_the_failed_gate() {
        let pairing = json!({"committed":true,"transaction_id":"request-1"});
        let failure = setup_failure(
            fail(11, "service_state_unconfirmed"),
            &pairing,
            "service_runtime",
        );
        assert!(failure.committed);
        assert_eq!(failure.transaction_id.as_deref(), Some("request-1"));
        assert_eq!(failure.step, Some("service_runtime"));
    }

    #[test]
    fn setup_reads_the_nested_pairing_state_and_filters_internal_options() {
        assert!(pairing_is_active(&json!({"pairing":{"state":"active"}})));
        assert!(!pairing_is_active(&json!({"state":"active"})));
        let parsed = Args::parse(
            vec![
                "setup".into(),
                "--interactive".into(),
                "--installer-session".into(),
                "--elevated-setup-child".into(),
            ],
            &["--bootstrap", "--file", "--server", "--expected-revision"],
            &["--network", "--sunshine"],
        )
        .unwrap();
        let child = setup_args(&parsed, vec!["pair".into()], true);
        assert!(child.has("--interactive"));
        assert!(!child.has("--installer-session"));
        assert!(!child.has("--elevated-setup-child"));
        assert!(
            child
                .validate_options(&["--interactive", "--input-stdin", "--server"])
                .is_ok()
        );
    }

    #[test]
    fn windows_sunshine_certificate_paths_cover_program_files_and_service_location() {
        let mut candidates = Vec::new();
        extend_windows_certificate_candidates(
            &mut candidates,
            Some(PathBuf::from("C:/Program Files")),
            Some(PathBuf::from("D:/Native Programs")),
            Some(PathBuf::from("E:/Custom Sunshine/sunshine.exe")),
        );
        assert!(candidates.contains(&PathBuf::from(
            "C:/Program Files/Sunshine/config/credentials/cacert.pem"
        )));
        assert!(candidates.contains(&PathBuf::from(
            "D:/Native Programs/Sunshine/config/credentials/cacert.pem"
        )));
        assert!(candidates.contains(&PathBuf::from(
            "E:/Custom Sunshine/config/credentials/cacert.pem"
        )));
    }

    #[test]
    fn unreadable_certificate_has_a_distinct_diagnostic() {
        assert_eq!(
            certificate_io_error(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied"
            ))
            .code,
            "sunshine_certificate_unreadable"
        );
    }

    #[test]
    fn compatibility_manifest_tracks_the_manager_protocol() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../compatibility.json")).unwrap();
        assert_eq!(manifest["sunshine_manager_protocol"], 1);
        assert_eq!(sunshine_client_protocol::PROTOCOL, "sunshine-management/1");
    }
}
