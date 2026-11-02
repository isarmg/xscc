use xscs_protocol::*;
#[test]
fn mobile_platforms_are_controllers_and_desktop_platforms_support_both_roles() {
    for (os, mobile) in [
        (ClientOs::LinuxX86_64, false),
        (ClientOs::LinuxAarch64, false),
        (ClientOs::LinuxArmv7, false),
        (ClientOs::WindowsX86_64, false),
        (ClientOs::MacosAarch64, false),
        (ClientOs::IosAarch64, true),
        (ClientOs::AndroidAarch64, true),
    ] {
        assert!(os.supports_role(InstanceRole::Controller));
        assert_eq!(os.supports_role(InstanceRole::Controlled), !mobile);
        let encoded = serde_json::to_string(&os).unwrap();
        assert_eq!(serde_json::from_str::<ClientOs>(&encoded).unwrap(), os);
    }
}
#[test]
fn accepts_only_embedded_rustdesk_and_current_contract() {
    let valid = serde_json::json!({"protocol":PROTOCOL,"client_version":"2.0.0-dev","os":"linux_x86_64","backend":"rustdesk","execution_mode":"embedded","diagnostics":true});
    let caps: Capabilities = serde_json::from_value(valid.clone()).unwrap();
    assert!(caps.validate().is_ok());
    for (field, value) in [
        ("backend", serde_json::json!("sunshine")),
        ("execution_mode", serde_json::json!("standalone")),
        ("protocol", serde_json::json!("xscs-management/1")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<Capabilities>(invalid)
                .map(|c| c.validate().is_err())
                .unwrap_or(true)
        );
    }
    for kind in [
        "save_config",
        "patch_config",
        "restart",
        "list_applications",
        "submit_pairing_pin",
        "control_service",
    ] {
        assert!(serde_json::from_value::<Command>(serde_json::json!({"kind":kind})).is_err())
    }
}
#[test]
fn readiness_requires_observed_native_version() {
    let mut snapshot = DiagnosticSnapshot::awaiting_integration();
    assert!(snapshot.validate().is_ok());
    snapshot.integration_state = IntegrationState::Ready;
    assert!(snapshot.validate().is_err());
    snapshot.rustdesk_version = Some("1.5.1-custom".into());
    assert!(snapshot.validate().is_ok());
    snapshot.rustdesk_version = Some("version\nforged-log".into());
    assert!(snapshot.validate().is_err());
}

#[test]
fn one_kvm_reports_its_own_version_and_preserves_rustdesk_wire_shape() {
    for os in [ClientOs::LinuxAarch64, ClientOs::LinuxArmv7] {
        let mut capabilities = Capabilities {
            protocol: PROTOCOL.into(),
            client_version: "0.2.8-custom".into(),
            os,
            backend: Backend::OneKvm,
            execution_mode: ExecutionMode::Embedded,
            diagnostics: true,
        };
        assert!(capabilities.validate().is_ok());
        capabilities.backend = Backend::Rustdesk;
        assert!(capabilities.validate().is_err());
    }
    let native = serde_json::to_value(DiagnosticSnapshot::awaiting_integration()).unwrap();
    assert!(native.get("one_kvm_version").is_none());
    let mut kvm = DiagnosticSnapshot {
        backend: Backend::OneKvm,
        execution_mode: ExecutionMode::Embedded,
        integration_state: IntegrationState::Ready,
        rustdesk_version: None,
        one_kvm_version: Some("0.2.8-custom".into()),
    };
    assert!(kvm.validate().is_ok());
    let encoded = serde_json::to_value(&kvm).unwrap();
    assert_eq!(encoded["backend"], "one_kvm");
    assert_eq!(encoded["one_kvm_version"], "0.2.8-custom");
    assert!(encoded["rustdesk_version"].is_null());
    kvm.rustdesk_version = Some("pretend-rustdesk".into());
    assert!(kvm.validate().is_err());
    kvm.rustdesk_version = None;
    kvm.one_kvm_version = None;
    assert!(kvm.validate().is_err());
}
