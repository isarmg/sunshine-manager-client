use crate::{
    cli_common::*,
    provisioning::{self, Bootstrap, Identity, ProvisionError},
    storage::{MaintenanceGuard, ProtectedState},
};
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairInput {
    server: String,
    pairing_code: Zeroizing<String>,
    sunshine_endpoint: String,
    sunshine_username: Zeroizing<String>,
    sunshine_password: Zeroizing<String>,
    #[serde(default)]
    restart_allowed: bool,
}
impl PairInput {
    fn bootstrap(self) -> Result<Bootstrap> {
        let mut url = sarmg_client_secure_http::Url::parse(&self.server).map_err(input_error)?;
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
        let b = Bootstrap {
            manager_endpoint: url.to_string(),
            enrollment_token: self.pairing_code,
            sunshine_endpoint: self.sunshine_endpoint,
            sunshine_username: self.sunshine_username,
            sunshine_password: self.sunshine_password,
            restart_allowed: self.restart_allowed,
        };
        b.validate().map_err(provision_error)?;
        Ok(b)
    }
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
    let mut options = args.options.clone();
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
    let mut options = args.options.clone();
    options.remove("--interactive");
    options.remove("--input-stdin");
    options.remove("--server");
    options.remove("--now");
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
                pairing_code: Zeroizing::new(prompt("Pairing code", true)?),
                sunshine_endpoint: prompt("Local Sunshine HTTPS URL", false)?,
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
    let _guard = MaintenanceGuard::acquire(path).map_err(storage_error)?;
    let store = ProtectedState::open(&path.join("provisioning")).map_err(storage_error)?;
    let existing = identity(&store)?;
    if resume && existing.is_none() {
        return Err(fail(4, "no_pairing_transaction"));
    }
    if let Some(b) = incoming {
        if let Some(id) = existing {
            if id.enrolled || id.config != b {
                return Err(fail(5, "binding_replacement_requires_retirement"));
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
    let existing = match read_store(&path).map_err(|error| error.at_step("configuration"))? {
        Some(store) => identity(&store).map_err(|error| error.at_step("configuration"))?,
        None => None,
    };
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
    if pairing_status["state"] != "active" {
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
            ask_yes_no("Enable service at system startup?", true)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
            ask_yes_no("Start the service now?", true)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
            ask_yes_no("Verify the connection now?", true)
                .map_err(|error| setup_failure(error, &pairing, "preferences"))?,
        )
    } else {
        (true, true, true)
    };
    let service_api = service();
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
        let status = service_api
            .verified_status(args.timeout, &path)
            .map_err(|error| setup_failure(error, &pairing, "service_runtime"))?;
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
        let status = wait_for_healthy(&path, args.timeout)
            .map_err(|error| setup_failure(error, &pairing, "connection"))?;
        record_setup_step(
            &mut steps,
            interactive,
            "connection",
            "verified",
            json!({"health":status["health"]}),
        );
        status
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
    let args = match Args::parse(raw) {
        Ok(a) => a,
        Err(e) => return emit("sunshine-client", "parse", "json", &Err(e)),
    };
    if args.has("--help") {
        println!(
            "sunshine-client: setup; config init|show|validate|diff|apply; pair [status|resume]; credentials update; status; doctor; service status|start|stop|restart|enable|disable; run; version\nGlobal: --format human|json|ndjson --non-interactive --timeout 60s --no-color --config ABSOLUTE_STATE_DIRECTORY (--state compatibility alias)\nsetup/pair uses --interactive or --input-stdin; setup completes pairing, service startup policy and connection verification. No secret arguments. Configuration apply requires --expected-revision. Services must be stopped for writes."
        );
        return 0;
    }
    if args.words.is_empty() && !args.has("--version") {
        return no_args(&args);
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
    emit("sunshine-client", &command, &args.format, &result)
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
            args.validate_options(&["--interactive", "--input-stdin", "--server"])?;
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
                value["network"] = provisioning::network_probe(&config).map_err(provision_error)?;
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
                value["sunshine"] =
                    json!({"reachable":observation.is_ok(),"trust_context":"current_cli_account"});
                if observation.is_err() {
                    return Err(fail(6, "sunshine_unavailable_or_untrusted"));
                }
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
                Credentials {
                    sunshine_username: Zeroizing::new(prompt("Sunshine username", false)?),
                    sunshine_password: Zeroizing::new(prompt("Sunshine password", true)?),
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
        ["pair"] | ["pair", "resume"] => execute_pair(args, &path),
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
}
