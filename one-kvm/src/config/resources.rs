use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use super::{AppConfig, AtxDriverType, HidBackend};
use crate::error::{AppError, Result};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Resource {
    kind: &'static str,
    device: Option<String>,
}

impl Resource {
    fn conflicts(&self, other: &Self) -> bool {
        self.kind == other.kind
            && (self.device.is_none() || other.device.is_none() || self.device == other.device)
    }

    fn label(&self) -> String {
        match &self.device {
            Some(device) => format!("{} {device}", self.kind),
            None => format!("{} automatic device selection", self.kind),
        }
    }
}

#[derive(Default)]
struct Claims {
    instances: BTreeSet<String>,
    resources: BTreeMap<Resource, (String, usize)>,
}

/// Shared only by the independent instances of one running One-KVM host.
#[derive(Default)]
pub struct ResourceRegistry {
    claims: Mutex<Claims>,
}

pub(crate) struct ResourceLease {
    registry: Arc<ResourceRegistry>,
    instance: String,
    current: Mutex<BTreeSet<Resource>>,
    retained: Mutex<BTreeSet<Resource>>,
}

/// Keeps candidate resources reserved until persistence or hardware apply ends.
/// Dropping a failed/cancelled operation retains the instance's previous claims.
pub struct ResourceReservation {
    lease: Arc<ResourceLease>,
    resources: Option<BTreeSet<Resource>>,
}

impl ResourceRegistry {
    pub fn reserve_listener(self: &Arc<Self>, port: u16) -> Result<ResourceReservation> {
        self.register("@web-listener".into())?
            .reserve_set(BTreeSet::from([Resource {
                kind: "TCP port",
                device: Some(port.to_string()),
            }]))
    }

    pub(crate) fn register(self: &Arc<Self>, instance: String) -> Result<Arc<ResourceLease>> {
        let mut claims = self.claims.lock().expect("resource registry lock poisoned");
        if !claims.instances.insert(instance.clone()) {
            return Err(AppError::Conflict(format!(
                "Instance {instance} is already running"
            )));
        }
        Ok(Arc::new(ResourceLease {
            registry: self.clone(),
            instance,
            current: Mutex::new(BTreeSet::new()),
            retained: Mutex::new(BTreeSet::new()),
        }))
    }
}

impl ResourceLease {
    pub(crate) fn reserve(self: &Arc<Self>, config: &AppConfig) -> Result<ResourceReservation> {
        if config.hid.backend == HidBackend::Ch9329
            && config.atx.enabled
            && config.atx.driver == AtxDriverType::Serial
            && device_id(&config.hid.ch9329_port) == device_id(&config.atx.device)
        {
            return Err(AppError::BadRequest(
                "ATX and keyboard/mouse cannot use the same serial device".into(),
            ));
        }
        self.reserve_set(config_resources(config))
    }

    fn reserve_set(self: &Arc<Self>, resources: BTreeSet<Resource>) -> Result<ResourceReservation> {
        let mut claims = self
            .registry
            .claims
            .lock()
            .expect("resource registry lock poisoned");
        if !self
            .retained
            .lock()
            .expect("resource quarantine lock poisoned")
            .is_empty()
        {
            return Err(AppError::ServiceUnavailable(
                "Device cleanup failed; stop this One-KVM instance before reconfiguring it".into(),
            ));
        }
        for requested in &resources {
            if let Some((_, (owner, _))) = claims.resources.iter().find(|(claimed, (owner, _))| {
                owner != &self.instance && claimed.conflicts(requested)
            }) {
                return Err(AppError::Conflict(format!(
                    "{} is assigned to instance {owner}",
                    requested.label()
                )));
            }
        }
        for resource in &resources {
            claims
                .resources
                .entry(resource.clone())
                .and_modify(|(_, count)| *count += 1)
                .or_insert_with(|| (self.instance.clone(), 1));
        }
        Ok(ResourceReservation {
            lease: self.clone(),
            resources: Some(resources),
        })
    }
}

impl ResourceReservation {
    /// Failed cleanup cannot make a possibly active device available to another
    /// instance. Keep its claims until this instance is stopped and dropped.
    pub(crate) fn quarantine(&mut self) {
        let Some(resources) = self.resources.take() else {
            return;
        };
        let mut claims = self
            .lease
            .registry
            .claims
            .lock()
            .expect("resource registry lock poisoned");
        let mut retained = self
            .lease
            .retained
            .lock()
            .expect("resource quarantine lock poisoned");
        for resource in resources {
            if !retained.insert(resource.clone()) {
                release(&mut claims, &BTreeSet::from([resource]));
            }
        }
    }

    pub(crate) fn commit(mut self) {
        let resources = self
            .resources
            .take()
            .expect("invariant: uncommitted reservation");
        let mut claims = self
            .lease
            .registry
            .claims
            .lock()
            .expect("resource registry lock poisoned");
        let mut current = self
            .lease
            .current
            .lock()
            .expect("resource lease lock poisoned");
        release(&mut claims, &current);
        *current = resources;
    }
}

impl Drop for ResourceReservation {
    fn drop(&mut self) {
        if let Some(resources) = &self.resources {
            let mut claims = self
                .lease
                .registry
                .claims
                .lock()
                .expect("resource registry lock poisoned");
            release(&mut claims, resources);
        }
    }
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        let mut claims = self
            .registry
            .claims
            .lock()
            .expect("resource registry lock poisoned");
        release(
            &mut claims,
            self.current
                .get_mut()
                .expect("resource lease lock poisoned"),
        );
        release(
            &mut claims,
            self.retained
                .get_mut()
                .expect("resource quarantine lock poisoned"),
        );
        claims.instances.remove(&self.instance);
    }
}

fn release(claims: &mut Claims, resources: &BTreeSet<Resource>) {
    for resource in resources {
        if let Some((_, count)) = claims.resources.get_mut(resource) {
            *count -= 1;
            if *count == 0 {
                claims.resources.remove(resource);
            }
        }
    }
}

fn device_id(value: &str) -> String {
    let value = value.trim();
    if cfg!(windows) {
        value.trim_start_matches(r"\\.\").to_lowercase()
    } else if value.starts_with('/') {
        std::fs::canonicalize(value)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}

fn audio_device_id(value: &str) -> Option<String> {
    let value = value.trim();
    if cfg!(windows) {
        // Only IMMDevice endpoint IDs identify a device independently of its label.
        let (endpoint, guid) = value.strip_prefix("wasapi:")?.split_once("}.{")?;
        if !matches!(endpoint, "{0.0.0.00000000" | "{0.0.1.00000000") {
            return None;
        }
        let guid = guid.strip_suffix('}')?;
        let canonical = uuid::Uuid::parse_str(guid).ok()?.hyphenated().to_string();
        if !guid.eq_ignore_ascii_case(&canonical) {
            return None;
        }
        Some(format!("wasapi:{endpoint}}}.{{{canonical}}}"))
    } else {
        // Named ALSA plugins and CARD aliases may resolve to these same numeric IDs.
        let device = value
            .strip_prefix("hw:")
            .or_else(|| value.strip_prefix("plughw:"))?;
        let (card, device) = device.split_once(',')?;
        if card.is_empty()
            || device.is_empty()
            || !card.bytes().all(|byte| byte.is_ascii_digit())
            || !device.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(format!(
            "hw:{},{}",
            card.parse::<u32>().ok()?,
            device.parse::<u32>().ok()?
        ))
    }
}

fn config_resources(config: &AppConfig) -> BTreeSet<Resource> {
    let mut resources = BTreeSet::new();
    let mut add = |kind, device: Option<String>| {
        resources.insert(Resource { kind, device });
    };
    match config.video.device.as_deref().map(str::trim) {
        Some(device)
            if device.eq_ignore_ascii_case("auto") || device.eq_ignore_ascii_case("default") =>
        {
            add("video", None)
        }
        Some("") => {}
        Some(device) => add("video", Some(device_id(device))),
        None => {}
    }
    if config.audio.enabled {
        // Legacy labels/plugins retain their behavior, with conservative exclusive ownership.
        add("audio", audio_device_id(&config.audio.device));
    }
    match config.hid.backend {
        HidBackend::Otg => add("USB gadget", Some("one-kvm".into())),
        HidBackend::Ch9329 => add("serial", Some(device_id(&config.hid.ch9329_port))),
        HidBackend::Bluetooth => add(
            "Bluetooth adapter",
            Some(config.hid.bluetooth.adapter.clone()),
        ),
        HidBackend::Windows => {
            if config.hid.windows_keyboard || config.hid.windows_mouse {
                add("Windows desktop input", None);
            }
        }
        HidBackend::None => {}
    }
    if config.msd.enabled || config.uac.enabled || config.otg_network.enabled {
        add("USB gadget", Some("one-kvm".into()));
    }
    if config.atx.enabled {
        match config.atx.driver {
            AtxDriverType::Serial => add("serial", Some(device_id(&config.atx.device))),
            AtxDriverType::UsbRelay => add("USB relay", Some(device_id(&config.atx.device))),
            AtxDriverType::Gpio => {
                for (enabled, device, pin) in [
                    (
                        config.atx.power.enabled,
                        &config.atx.power.device,
                        config.atx.power.pin,
                    ),
                    (
                        config.atx.reset.enabled,
                        &config.atx.reset.device,
                        config.atx.reset.pin,
                    ),
                    (
                        config.atx.led.enabled,
                        &config.atx.led.device,
                        config.atx.led.pin,
                    ),
                    (
                        config.atx.hdd.enabled,
                        &config.atx.hdd.device,
                        config.atx.hdd.pin,
                    ),
                ] {
                    if enabled {
                        let device = if device.is_empty() {
                            &config.atx.device
                        } else {
                            device
                        };
                        add("GPIO", Some(format!("{}:{pin}", device_id(device))));
                    }
                }
            }
            AtxDriverType::None => {}
        }
    }
    if config.rustdesk.enabled
        && config.rustdesk.mode == crate::rustdesk::config::RustDeskMode::DirectIp
    {
        add(
            "TCP port",
            Some(config.rustdesk.direct_access_port.to_string()),
        );
    }
    if config.vnc.enabled {
        add("TCP port", Some(config.vnc.port.to_string()));
    }
    if config.rtsp.enabled {
        add("TCP port", Some(config.rtsp.port.to_string()));
    }
    resources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_cleanup_keeps_candidate_devices_reserved_until_the_instance_is_dropped() {
        let registry = Arc::new(ResourceRegistry::default());
        let a = registry.register("a".into()).unwrap();
        let b = registry.register("b".into()).unwrap();
        let mut config = AppConfig::default();
        config.enforce_invariants();
        config.video.device = Some("camera-a".into());
        a.reserve(&config).unwrap().commit();
        config.video.device = Some("camera-b".into());
        {
            let mut failed = a.reserve(&config).unwrap();
            let mut duplicate = a.reserve(&config).unwrap();
            failed.quarantine();
            duplicate.quarantine();
        }
        assert!(matches!(b.reserve(&config), Err(AppError::Conflict(_))));
        config.video.device = Some("camera-a".into());
        assert!(matches!(b.reserve(&config), Err(AppError::Conflict(_))));
        assert!(matches!(
            a.reserve(&config),
            Err(AppError::ServiceUnavailable(_))
        ));
        drop(a);
        b.reserve(&config).unwrap().commit();
        config.video.device = Some("camera-b".into());
        b.reserve(&config).unwrap().commit();
    }

    #[test]
    fn exclusive_devices_are_released_on_replace_and_failed_apply() {
        let registry = Arc::new(ResourceRegistry::default());
        let a = registry.register("a".into()).unwrap();
        let b = registry.register("b".into()).unwrap();
        let mut config = AppConfig::default();
        config.enforce_invariants();
        config.video.device = Some("capture-a".into());
        a.reserve(&config).unwrap().commit();
        assert!(matches!(b.reserve(&config), Err(AppError::Conflict(_))));
        config.video.device = Some("capture-b".into());
        let pending = a.reserve(&config).unwrap();
        let mut unrelated_change = config.clone();
        unrelated_change.video.device = Some("capture-a".into());
        unrelated_change.audio.quality = "voice".into();
        a.reserve(&unrelated_change).unwrap().commit();
        assert!(b.reserve(&config).is_err());
        drop(pending);
        b.reserve(&config).unwrap().commit();
        config.video.device = Some("capture-a".into());
        assert!(b.reserve(&config).is_err());
        drop(a);
        b.reserve(&config).unwrap().commit();
    }

    #[test]
    fn automatic_sources_and_shared_otg_input_cannot_cross_instances() {
        let registry = Arc::new(ResourceRegistry::default());
        let a = registry.register("a".into()).unwrap();
        let b = registry.register("b".into()).unwrap();
        let mut config = AppConfig::default();
        config.enforce_invariants();
        config.video.device = Some("auto".into());
        a.reserve(&config).unwrap().commit();
        config.video.device = Some("capture-a".into());
        assert!(b.reserve(&config).is_err());
        config.video.device = None;
        config.hid.backend = HidBackend::Otg;
        b.reserve(&config).unwrap().commit();
        config.hid.otg_udc = Some("another-udc".into());
        assert!(a.reserve(&config).is_err());
    }

    #[test]
    fn shared_web_port_and_windows_desktop_input_are_exclusive() {
        let registry = Arc::new(ResourceRegistry::default());
        let _web = registry.reserve_listener(8080).unwrap();
        let a = registry.register("a".into()).unwrap();
        let b = registry.register("b".into()).unwrap();
        let mut config = AppConfig::default();
        config.enforce_invariants();
        config.rustdesk.enabled = true;
        config.rustdesk.mode = crate::rustdesk::config::RustDeskMode::DirectIp;
        config.rustdesk.direct_access_port = 8080;
        assert!(a.reserve(&config).is_err());
        config.rustdesk.enabled = false;
        config.hid.backend = HidBackend::Windows;
        config.hid.windows_keyboard = true;
        config.hid.windows_monitor = r"desktop:\\.\DISPLAY1".into();
        a.reserve(&config).unwrap().commit();
        config.hid.windows_monitor = r"desktop:\\.\DISPLAY2".into();
        assert!(b.reserve(&config).is_err());
    }

    #[test]
    fn legacy_audio_alias_cannot_bypass_an_enumerated_device_assignment() {
        let registry = Arc::new(ResourceRegistry::default());
        let a = registry.register("a".into()).unwrap();
        let b = registry.register("b".into()).unwrap();
        let mut config = AppConfig::default();
        config.enforce_invariants();
        config.audio.enabled = true;
        config.audio.device = if cfg!(windows) {
            "USB Audio Device"
        } else {
            "plughw:CARD=Capture,DEV=0"
        }
        .into();
        a.reserve(&config).unwrap().commit();
        config.audio.device = if cfg!(windows) {
            "wasapi:{0.0.1.00000000}.{ABCDEF01-2345-6789-ABCD-EF0123456789}"
        } else {
            "hw:1,0"
        }
        .into();
        assert!(audio_device_id(&config.audio.device).is_some());
        assert!(matches!(b.reserve(&config), Err(AppError::Conflict(_))));
        drop(a);
        b.reserve(&config).unwrap().commit();
        let a = registry.register("a".into()).unwrap();
        config.audio.device = if cfg!(windows) {
            "USB Audio Device"
        } else {
            "plughw:CARD=Capture,DEV=0"
        }
        .into();
        assert!(matches!(a.reserve(&config), Err(AppError::Conflict(_))));
    }
}
