use super::topology::Topology;
use super::*;
use base::session_protocol::virtual_display::{
    VirtualDisplayConfiguration, VirtualDisplayMode, VirtualDisplayState, PROTOCOL_VERSION,
};

fn policy(folder: &Path) -> ResultType<Option<VirtualDisplayConfiguration>> {
    let path = folder.join("configuration.json");
    if !path.exists() {
        return Ok(None);
    }
    let config: VirtualDisplayConfiguration =
        serde_json::from_slice(&bounded(&path, 1024 * 1024)?)?;
    config.validate()?;
    Ok(Some(config))
}

pub(super) fn capture_idle_for_change(
) -> ResultType<crate::server::video_service::CaptureIdleGuard<'static>> {
    crate::server::video_service::wait_for_capture_idle(Duration::from_secs(5)).map_err(|cause| {
        anyhow::anyhow!("屏幕捕获未能安全释放，显示设备未更改；请断开远控会话后重试：{cause}")
    })
}

pub(super) fn automatic_headless_allowed() -> ResultType<bool> {
    let state = query()?;
    if state.status == "external_owned" {
        bail!("{}", state.message);
    }
    Ok(policy(&bundle_path()?.join("config"))?.is_none_or(|config| config.count > 0))
}

pub(super) fn present_count() -> ResultType<usize> {
    if device()?.is_none() || problem()? == CM_PROB_DISABLED {
        return Ok(0);
    }
    Ok(topology::monitor_ids()?.len())
}

fn display_query_error(error: anyhow::Error, problem: u32) -> anyhow::Error {
    if problem == 0 {
        return error;
    }
    // Remote restart classification still reads the original top-level error text.
    let message = format!("{error} (previously observed VDD device problem code: {problem})");
    error.context(message)
}

pub(super) fn query() -> ResultType<VirtualDisplayState> {
    let bundle = bundle_path()?;
    let folder = bundle.join("config");
    let installed = device()?.is_some();
    let registered = registered_path()?;
    let mut state = VirtualDisplayState {
        protocol_version: PROTOCOL_VERSION,
        driver: "mtt_vdd".into(),
        status: if installed { "ready" } else { "not_installed" }.into(),
        message: String::new(),
        ..Default::default()
    };
    if registered.as_ref().is_some_and(|path| {
        !path
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(&ordinary_path(&folder))
    }) || installed && (!folder.join("owner.txt").exists() || registered.is_none())
    {
        state.status = "external_owned".into();
        state.message = "现有虚拟显示驱动由其他应用管理，XSCC 不会接管。".into();
        return Ok(state);
    }
    if folder.exists() {
        check_path(&folder, true)?;
        if bounded(&folder.join("owner.txt"), 128)? != OWNER {
            state.status = "external_owned".into();
            state.message = "虚拟显示配置所有权不属于 XSCC。".into();
            return Ok(state);
        }
        if let Some(config) = policy(&folder)? {
            state.persistent = config.persistent;
            state.configured_count = config.count;
        }
    }
    let profile = if folder.join("vdd_settings.xml").exists() {
        folder.join("vdd_settings.xml")
    } else {
        bundle.join("vdd_settings.xml")
    };
    let profile = Profile::parse(String::from_utf8(bounded(&profile, 1024 * 1024)?)?)
        .map_err(anyhow::Error::msg)?;
    state.modes = profile
        .modes
        .iter()
        .map(|mode| VirtualDisplayMode {
            width: mode.width,
            height: mode.height,
            refresh_rate_units: mode.refresh_units,
        })
        .collect();
    if !installed {
        state.message = "驱动随程序提供，应用配置时将按需安装。".into();
        return Ok(state);
    }
    let problem = problem()?;
    if policy(&folder)?.is_none() {
        state.configured_count = if problem == CM_PROB_DISABLED {
            0
        } else {
            profile.monitor_count
        };
    }
    let ids = topology::monitor_ids().map_err(|error| display_query_error(error, problem))?;
    state.displays = Topology::query(true)
        .map_err(|error| display_query_error(error, problem))?
        .screens(&ids)
        .map_err(|error| display_query_error(error, problem))?;
    state.present_count = u32::try_from(ids.len())?;
    state.active_count = u32::try_from(
        state
            .displays
            .iter()
            .filter(|display| display.active)
            .count(),
    )?;
    if problem == CM_PROB_DISABLED {
        state.status = "disabled".into();
        state.message = "虚拟显示驱动已停用。应用非零数量可重新启用。".into();
    } else if problem == 14 || restart_required()? {
        state.status = "reboot_required".into();
        state.message = "Windows 要求重启后继续使用虚拟显示驱动。".into();
    } else if problem != 0 {
        state.status = "error".into();
        state.message = format!("Windows 虚拟显示设备故障，代码 {problem}。");
    } else if state.configured_count == 0 && state.present_count > 0 {
        state.status = "error".into();
        state.message = "虚拟显示器移除尚未完成，请先断开远控会话，再在被控端重试移除。".into();
    } else {
        state.message = "数量与当前模式由 Windows 实际枚举确认；显卡最大虚拟屏数未知。".into();
    }
    Ok(state)
}

pub(crate) fn state() -> VirtualDisplayState {
    query().unwrap_or_else(|error| VirtualDisplayState {
        protocol_version: PROTOCOL_VERSION,
        driver: "mtt_vdd".into(),
        status: "error".into(),
        message: format!("{error:#}"),
        ..Default::default()
    })
}

pub(crate) fn prepare() -> ResultType<VirtualDisplayState> {
    let started = Instant::now();
    let _guard = LOCK.lock().unwrap();
    let state = query()?;
    if state.status == "external_owned" {
        bail!("{}", state.message)
    }
    let bundle = verified_bundle()?;
    if device()?.is_none() {
        let _capture_idle = capture_idle_for_change()?;
        configure_count_modes_with_verified_bundle(&bundle, 1, &[], started)?;
        configure_count_modes_with_verified_bundle(&bundle, 0, &[], started)?;
    }
    query()
}

pub(crate) fn configure(config: &VirtualDisplayConfiguration) -> ResultType<VirtualDisplayState> {
    config.validate()?;
    let started = Instant::now();
    hbb_common::throttled_log!(
        Duration::from_secs(1),
        info,
        "vdd_config operation={started:?} count={} stage=begin",
        config.count
    );
    let _guard = LOCK.lock().unwrap();
    let before = query()?;
    if before.status == "external_owned" {
        bail!("{}", before.message)
    }
    if config.count > before.configured_count && config.count - before.configured_count > 32 {
        bail!("为避免一次创建大量设备，单次最多新增 32 块虚拟屏，请分批应用；这不是显卡数量上限");
    }
    if config.count == 0 {
        let active_virtual = before
            .displays
            .iter()
            .filter(|screen| screen.active)
            .count();
        if windows::get_device_names(None).len() <= active_virtual && active_virtual > 0 {
            bail!("不能删除当前仅剩的显示器，请先启用一块其他显示器");
        }
    }
    let verification_started = Instant::now();
    let bundle = verified_bundle()?;
    hbb_common::throttled_log!(
        Duration::from_secs(1),
        info,
        "vdd_config operation={started:?} count={} stage=bundle_verified elapsed_ms={} stage_ms={}",
        config.count,
        started.elapsed().as_millis(),
        verification_started.elapsed().as_millis()
    );
    let present = device()?.is_some();
    let enabled = present && problem()? != CM_PROB_DISABLED;
    let folder = owned_folder(&bundle, present)?;
    let profile_path = folder.join("vdd_settings.xml");
    let previous = if profile_path.exists() {
        Some(bounded(&profile_path, 1024 * 1024)?)
    } else {
        None
    };
    let policy_path = folder.join("configuration.json");
    let modes_match = config.modes.iter().all(|mode| before.modes.contains(mode))
        && config.modes.first().is_none_or(|mode| {
            before
                .displays
                .iter()
                .filter(|screen| screen.active)
                .all(|screen| {
                    screen.width == mode.width
                        && screen.height == mode.height
                        && screen.refresh_rate_units.abs_diff(mode.refresh_rate_units) <= 1
                })
        });
    if config.count == before.configured_count
        && before.active_count == config.count
        && before.present_count == config.count
        && modes_match
    {
        if config.persistent && presentation::active() {
            let _capture_idle = capture_idle_for_change()?;
            presentation::suspend_locked()?;
        }
        write_profile(&policy_path, &serde_json::to_string(config)?)?;
        return query();
    }
    if config.count == 0 {
        let _capture_idle = capture_idle_for_change()?;
        let presentation = presentation::suspend_locked();
        // Record the requested removal before PnP notifications can trigger a late login.
        let removal = (|| -> ResultType<()> {
            write_profile(&policy_path, &serde_json::to_string(config)?)?;
            configure_count_modes_with_verified_bundle(&bundle, 0, &[], started)
        })();
        presentation::complete_removal(presentation, removal)?;
        return query();
    }
    let previous_policy = if policy_path.exists() {
        Some(bounded(&policy_path, 1024 * 1024)?)
    } else {
        None
    };
    let registered = registered_path()?;
    let modes: Vec<_> = config
        .modes
        .iter()
        .map(|mode| {
            let mode = Mode::new(mode.width, mode.height, mode.refresh_rate_units)
                .map_err(anyhow::Error::msg)?;
            mode.validate_driver_arithmetic()
                .map_err(anyhow::Error::msg)?;
            Ok(mode)
        })
        .collect::<ResultType<_>>()?;
    let capture_started = Instant::now();
    let _capture_idle = capture_idle_for_change()?;
    hbb_common::throttled_log!(
        Duration::from_secs(1),
        info,
        "vdd_config operation={started:?} count={} stage=capture_idle elapsed_ms={} stage_ms={}",
        config.count,
        started.elapsed().as_millis(),
        capture_started.elapsed().as_millis()
    );
    let presentation = presentation::suspend_locked()?;
    let snapshot = match Topology::query(false) {
        Ok(snapshot) => snapshot,
        Err(cause) => {
            if let Some(target) = presentation {
                if let Err(error) = presentation::resume_locked(target) {
                    return Err(cause.context(format!("高刷会话布局未恢复：{error:#}")));
                }
            }
            return Err(cause);
        }
    };
    let apply = (|| -> ResultType<VirtualDisplayState> {
        configure_count_modes_with_verified_bundle(&bundle, config.count, &modes, started)?;
        if config.count > 0 {
            let ccd_started = Instant::now();
            let ids = topology::monitor_ids()?;
            let preferred = config
                .modes
                .first()
                .copied()
                .or_else(|| {
                    before
                        .displays
                        .iter()
                        .find(|screen| {
                            screen.active
                                && screen.width > 0
                                && screen.height > 0
                                && screen.refresh_rate_units > 0
                        })
                        .map(|screen| VirtualDisplayMode {
                            width: screen.width,
                            height: screen.height,
                            refresh_rate_units: screen.refresh_rate_units,
                        })
                })
                .or_else(|| before.modes.first().copied());
            hbb_common::throttled_log!(
                Duration::from_secs(1),
                info,
                "vdd_config operation={started:?} count={} stage=ccd_and_index_begin elapsed_ms={}",
                config.count,
                started.elapsed().as_millis()
            );
            // PnP, CCD, and capture enumerate on different Windows notifications.
            // Wait only inside this explicit operation; real topology rejections fail immediately.
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                match Topology::configure(&ids, preferred, config.modes.first().copied()) {
                    Ok(()) => break,
                    Err(error)
                        if error
                            .downcast_ref::<topology::MonitorPathPending>()
                            .is_some()
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(error) => return Err(error),
                }
            }
            loop {
                let actual = Topology::query(false)?.screens(&ids)?;
                let capturable = actual.len() == config.count as usize
                    && actual
                        .iter()
                        .all(|display| display.active && display.index.is_some());
                let mode_matches = config.modes.first().is_none_or(|mode| {
                    actual.iter().all(|display| {
                        display.width == mode.width
                            && display.height == mode.height
                            && display.refresh_rate_units.abs_diff(mode.refresh_rate_units) <= 1
                    })
                });
                if capturable && mode_matches {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    if !capturable {
                        bail!("Windows 尚未提供所请求数量的可捕获虚拟显示器");
                    }
                    let display = actual
                        .iter()
                        .find(|display| {
                            config.modes.first().is_some_and(|mode| {
                                display.width != mode.width
                                    || display.height != mode.height
                                    || display.refresh_rate_units.abs_diff(mode.refresh_rate_units)
                                        > 1
                            })
                        })
                        .ok_or_else(|| anyhow::anyhow!("Windows 尚未确认请求的虚拟显示模式"))?;
                    bail!(
                        "Windows 实际应用 {}×{} @{}Hz，与请求模式不同",
                        display.width,
                        display.height,
                        f64::from(display.refresh_rate_units) / 10000.
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            hbb_common::throttled_log!(
                Duration::from_secs(1), info,
                "vdd_config operation={started:?} count={} stage=ccd_and_index_confirmed elapsed_ms={} stage_ms={}",
                config.count, started.elapsed().as_millis(), ccd_started.elapsed().as_millis()
            );
        }
        write_profile(&policy_path, &serde_json::to_string(config)?)?;
        query()
    })();
    match apply {
        Ok(state) => {
            if let Some(target) = presentation {
                presentation::resume_locked(target)?;
            }
            hbb_common::throttled_log!(
                Duration::from_secs(1), info,
                "vdd_config operation={started:?} count={} stage=return_ready_for_ack elapsed_ms={}",
                config.count, started.elapsed().as_millis()
            );
            Ok(state)
        }
        Err(cause) => {
            hbb_common::throttled_log!(
                Duration::from_secs(1), warn,
                "vdd_config operation={started:?} count={} stage=apply_failed elapsed_ms={} error={cause:#}",
                config.count, started.elapsed().as_millis()
            );
            let mut errors = Vec::new();
            let owned = ordinary_path(&folder);
            let restore_path =
                registered
                    .as_deref()
                    .or(if !present { Some(owned.as_str()) } else { None });
            if let Err(error) =
                restore_configuration(&profile_path, previous.as_deref(), restore_path)
            {
                errors.push(format!("restore profile: {error:#}"));
            }
            if let Err(error) = (|| -> ResultType<()> {
                if device()?.is_some() {
                    change_device(if enabled { DICS_PROPCHANGE } else { DICS_DISABLE })?;
                }
                snapshot.restore()?;
                if let Some(bytes) = previous_policy {
                    write_profile(&policy_path, std::str::from_utf8(&bytes)?)?;
                } else if policy_path.exists() {
                    check_path(&policy_path, false)?;
                    fs::remove_file(&policy_path)?;
                }
                Ok(())
            })() {
                errors.push(format!("restore device/topology: {error:#}"));
            }
            if errors.is_empty() {
                if let Some(target) = presentation {
                    if let Err(error) = presentation::resume_locked(target) {
                        return Err(
                            cause.context(format!("原配置已恢复，但高刷会话布局未恢复：{error:#}"))
                        );
                    }
                }
                return Err(cause.context("虚拟屏配置未完成，原配置和显示拓扑已恢复"));
            }
            bail!(
                "虚拟屏配置未完成：{cause:#}；恢复失败：{}",
                errors.join("; ")
            );
        }
    }
}

fn temporary_displays_need_cleanup(state: &VirtualDisplayState) -> bool {
    state.status != "external_owned"
        && !state.persistent
        && (state.configured_count > 0 || state.present_count > 0)
}

pub(crate) fn cleanup_required_at_exit() -> ResultType<bool> {
    let _guard = LOCK.lock().unwrap();
    if device()?.is_none() {
        return Ok(false);
    }
    let state = query()?;
    Ok(temporary_displays_need_cleanup(&state))
}

pub(super) fn cleanup_session() -> ResultType<()> {
    let _guard = LOCK.lock().unwrap();
    let presentation = presentation::suspend_locked();
    let removal = cleanup_displays();
    presentation::complete_removal(presentation, removal)
}

fn cleanup_displays() -> ResultType<()> {
    let state = query()?;
    if state.status == "external_owned" || state.persistent || device()?.is_none() {
        return Ok(());
    }
    let active_virtual = state.displays.iter().filter(|screen| screen.active).count();
    if windows::get_device_names(None).len() <= active_virtual {
        return Ok(());
    }
    let folder = bundle_path()?.join("config");
    let mut config = policy(&folder)?.unwrap_or(VirtualDisplayConfiguration {
        count: 0,
        modes: Vec::new(),
        persistent: false,
    });
    config.count = 0;
    write_profile(
        &folder.join("configuration.json"),
        &serde_json::to_string(&config)?,
    )?;
    configure_count_modes(0, &[])
}

fn restart_required() -> ResultType<bool> {
    let Some((devices, mut data)) = device()? else {
        return Ok(false);
    };
    let mut parameters: SP_DEVINSTALL_PARAMS_W = unsafe { std::mem::zeroed() };
    parameters.cbSize = std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as _;
    if unsafe { SetupDiGetDeviceInstallParamsW(devices.0, &mut data, &mut parameters) } == FALSE {
        bail!(
            "Read VDD restart status failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(parameters.Flags & (DI_NEEDRESTART | DI_NEEDREBOOT) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_displays_need_exit_cleanup_without_primary_preference() {
        for (configured_count, present_count) in [(1, 1), (1, 0), (0, 1)] {
            let state = VirtualDisplayState {
                configured_count,
                present_count,
                ..Default::default()
            };
            assert!(temporary_displays_need_cleanup(&state));
        }
    }

    #[test]
    fn exit_cleanup_preserves_persistent_external_and_empty_displays() {
        let mut state = VirtualDisplayState {
            configured_count: 1,
            present_count: 1,
            persistent: true,
            ..Default::default()
        };
        assert!(!temporary_displays_need_cleanup(&state));
        state.persistent = false;
        state.status = "external_owned".into();
        assert!(!temporary_displays_need_cleanup(&state));
        state.status = "disabled".into();
        state.configured_count = 0;
        state.present_count = 0;
        assert!(!temporary_displays_need_cleanup(&state));
    }

    #[test]
    fn display_query_failure_retains_the_observed_problem_and_original_cause() {
        for message in ["Display query failed", "Display query requires a restart"] {
            for problem in [0, 14, 22, 43] {
                let cause =
                    anyhow::Error::new(std::io::Error::from_raw_os_error(5)).context(message);
                let root = cause.root_cause().to_string();
                let error = display_query_error(cause, problem);
                let text = error.to_string();
                assert!(text.starts_with(message));
                assert_eq!(text.contains("restart"), message.contains("restart"));
                assert!(format!("{error:#}").contains(&root));
                let source = error.downcast_ref::<std::io::Error>().unwrap();
                assert_eq!(source.raw_os_error(), Some(5));
                if problem == 0 {
                    assert_eq!(text, message);
                } else {
                    assert!(text.contains(&format!("problem code: {problem}")));
                }
            }
        }
    }
}
