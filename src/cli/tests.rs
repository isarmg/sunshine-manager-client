use super::*;

#[test]
fn setup_interactive_autostart_defaults_to_yes_when_enter_is_pressed() {
    let mut prompts = 0;
    let enabled = setup_autostart(true, |label, default| {
        prompts += 1;
        assert_eq!(label, "Enable service at system startup?");
        assert!(default);
        confirmation_answer("", default)
    })
    .unwrap();
    assert!(enabled);
    assert_eq!(prompts, 1);
}

#[test]
fn setup_without_interactive_input_enables_autostart_without_reading_stdin() {
    assert!(
        setup_autostart(false, |_, _| {
            panic!("non-interactive Setup must not consume another input document")
        })
        .unwrap()
    );
}

#[test]
fn setup_respects_an_explicit_autostart_decline_and_rejects_invalid_answers() {
    assert!(!setup_autostart(true, |_, default| confirmation_answer("no", default)).unwrap());
    let failure =
        setup_autostart(true, |_, default| confirmation_answer("later", default)).unwrap_err();
    assert_eq!(failure.code, "invalid_confirmation");
    assert_eq!(failure.exit, 2);
}

#[test]
fn product_input_paths_are_validated_independently_of_common_options() {
    let root = std::env::current_dir().unwrap();
    for option in ["--file", "--bootstrap"] {
        for path in [
            PathBuf::from("candidate.json"),
            root.join("../candidate.json"),
        ] {
            let mut args = Args::parse(Vec::new(), &[], &[]).unwrap();
            args.options
                .insert(option.into(), path.display().to_string());
            let error = validate_product_paths(args)
                .err()
                .expect("unsafe path rejected");
            assert_eq!(error.code, "absolute_path_required");
            assert_eq!(error.exit, 2);
        }
        let mut args = Args::parse(Vec::new(), &[], &[]).unwrap();
        args.options.insert(
            option.into(),
            root.join("candidate.json").display().to_string(),
        );
        assert!(validate_product_paths(args).is_ok());
    }
}

fn installer_test_root(directory: &tempfile::TempDir) -> PathBuf {
    #[cfg(windows)]
    {
        // canonicalize() produces a \\?\-prefixed VerbatimDisk path on
        // Windows. Production storage deliberately accepts only a normal
        // local drive path, so keep tempfile's absolute DOS path here.
        directory.path().join("client")
    }
    #[cfg(unix)]
    {
        // macOS temp paths can traverse /var -> /private/var; resolve that
        // ancestor before the no-symlink protected-store checks run.
        directory.path().canonicalize().unwrap().join("client")
    }
}

#[test]
fn sunshine_owns_manager_and_local_api_error_presentation() {
    assert_eq!(
        SunshineErrorCatalog.message("pairing_endpoint_not_found"),
        Some("The configured xscs does not expose the required pairing endpoint.")
    );
    assert_eq!(
        SunshineErrorCatalog.message("sunshine_credentials_rejected"),
        Some("Local Sunshine rejected its Basic Authentication credentials.")
    );
    assert_eq!(
        SunshineErrorCatalog.message("invalid_authorization_code"),
        Some(
            "The Sunshine instance authorization code must be the exact 36-character code issued by the Manager."
        )
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
    assert!(!startup_policy_matches(
        &json!({"startup":"enabled-runtime"}),
        true
    ));
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
fn compatibility_manifest_tracks_the_manager_protocol() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../compatibility.json")).unwrap();
    assert_eq!(manifest["xscs_protocol"], 1);
    assert_eq!(xscs_protocol::PROTOCOL, "xscs-management/1");
}

#[test]
fn incompatible_account_and_important_data_have_distinct_recovery_contracts() {
    let account = provision_error(ProvisionError::StateDocumentCorrupt {
        artifact: "identity",
    });
    assert_eq!(account.exit, 4);
    assert_eq!(account.code, "pairing_state_incompatible");
    assert!(
        SunshineErrorCatalog
            .next_step("xscc", &account)
            .unwrap()
            .contains("pair replace --interactive")
    );

    let important = important_state_error("execution-journal");
    assert_eq!(important.exit, 10);
    assert_eq!(important.code, "important_state_incompatible");
    assert_eq!(
        important.detail.as_deref(),
        Some("artifact=execution-journal;preserved=true")
    );
    assert!(
        SunshineErrorCatalog
            .next_step("xscc", &important)
            .unwrap()
            .contains("Do not delete")
    );
}

#[test]
fn installer_cleanup_accepts_unknown_regular_old_data_but_rejects_links() {
    let directory = tempfile::tempdir().unwrap();
    let removable = directory.path().join("old-state");
    std::fs::create_dir(&removable).unwrap();
    std::fs::write(removable.join("unknown-v1.bin"), b"old bytes").unwrap();
    remove_installer_tree(&removable).unwrap();
    assert!(!removable.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let unsafe_tree = directory.path().join("unsafe-state");
        std::fs::create_dir(&unsafe_tree).unwrap();
        let outside = directory.path().join("outside");
        std::fs::write(&outside, b"must survive").unwrap();
        symlink(&outside, unsafe_tree.join("linked")).unwrap();
        assert!(remove_installer_tree(&unsafe_tree).is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"must survive");
    }
}

#[test]
fn installer_preparation_archives_only_incompatible_account_documents() {
    let directory = tempfile::tempdir().unwrap();
    let root = installer_test_root(&directory);
    crate::storage::prepare_root(&root).unwrap();
    let provisioning = root.join("provisioning");
    {
        let store = ProtectedState::open(&provisioning).unwrap();
        let legacy = br#"{"manager_endpoint":"https://manager.example/xscc/v1/"}"#;
        store.put("identity.json", legacy).unwrap();
        store.put("bootstrap.json", legacy).unwrap();
    }
    let result = installer_prepare_setup(&root).unwrap();
    let archived = result["archived"].as_array().unwrap();
    assert_eq!(archived.len(), 2);
    let store = ProtectedState::open_readonly(&provisioning).unwrap();
    assert!(store.read("identity.json").unwrap().is_none());
    assert!(store.read("bootstrap.json").unwrap().is_none());
    for name in archived {
        assert!(
            store
                .read(name.as_str().expect("archive name"))
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn installer_preparation_preserves_account_when_important_data_is_invalid() {
    let directory = tempfile::tempdir().unwrap();
    let root = installer_test_root(&directory);
    crate::storage::prepare_root(&root).unwrap();
    let provisioning = root.join("provisioning");
    {
        let store = ProtectedState::open(&provisioning).unwrap();
        store
            .put(
                "identity.json",
                br#"{"manager_endpoint":"https://manager.example/xscc/v1/"}"#,
            )
            .unwrap();
    }
    std::fs::write(root.join("journal"), b"not a journal directory").unwrap();

    let failure = installer_prepare_setup(&root).unwrap_err();
    assert_eq!(failure.code, "important_state_incompatible");
    let store = ProtectedState::open_readonly(&provisioning).unwrap();
    assert!(store.read("identity.json").unwrap().is_some());
}

#[test]
fn installer_preparation_leaves_current_account_documents_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let root = installer_test_root(&directory);
    crate::storage::prepare_root(&root).unwrap();
    let provisioning = root.join("provisioning");
    let current = br#"{"manager_endpoint":"wss://manager.example/xscc/v1/connect","enrollment_token":"token","sunshine_endpoint":"https://127.0.0.1:47990/","sunshine_username":"sunshine","sunshine_password":"password"}"#;
    {
        let store = ProtectedState::open(&provisioning).unwrap();
        store.put("bootstrap.json", current).unwrap();
    }

    let result = installer_prepare_setup(&root).unwrap();
    assert_eq!(result["archived"], json!([]));
    let store = ProtectedState::open_readonly(&provisioning).unwrap();
    assert_eq!(store.read("bootstrap.json").unwrap().unwrap(), current);
}
