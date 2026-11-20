use linux_video_device::{
    ioctl::{querycap, Capabilities, Capability, FormatIterator},
    nix::libc::O_NONBLOCK,
    QueueType,
};
use std::{env, fs, fs::OpenOptions, os::unix::fs::OpenOptionsExt, path::Path};

fn has_library(names: &[&str]) -> bool {
    names.iter().any(|name| {
        // These are fixed platform sonames. FFmpeg later opens the selected encoder.
        unsafe { libloading::os::unix::Library::open(Some(*name), libloading::os::unix::RTLD_LAZY) }
            .is_ok()
    })
}

pub(crate) fn supported_gpu() -> (bool, bool, bool) {
    let nvenc = has_library(&["libcuda.so.1", "libcuda.so"])
        && has_library(&["libnvidia-encode.so.1", "libnvidia-encode.so"]);
    let amf = if cfg!(target_pointer_width = "64") {
        has_library(&["libamfrt64.so.1"])
    } else {
        has_library(&["libamfrt32.so.1"])
    };
    let qsv = has_library(&[
        "libvpl.so.2",
        "libvpl.so",
        "libmfx.so.1",
        "libmfx.so",
        "libmfx-gen.so.1.2",
        "libmfxhw64.so.1",
    ]);
    (nvenc, amf, qsv)
}

pub(crate) fn supports_rkmpp() -> bool {
    [
        "/dev/mpp_service",
        "/dev/vpu_service",
        "/dev/vcodec_service",
    ]
    .into_iter()
    .any(|path| Path::new(path).exists())
}

fn platform_is_amlogic() -> bool {
    const HINTS: &[&str] = &["amlogic", "meson", "gxl-vdec", "gx-vdec"];
    let contains_hint = |path: &Path| {
        fs::read(path).is_ok_and(|bytes| {
            let value = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            HINTS.iter().any(|hint| value.contains(hint))
        })
    };

    [
        "/proc/device-tree/compatible",
        "/proc/device-tree/model",
        "/sys/firmware/devicetree/base/compatible",
        "/sys/firmware/devicetree/base/model",
    ]
    .into_iter()
    .any(|path| contains_hint(Path::new(path)))
        || fs::read_dir("/sys/class/video4linux").is_ok_and(|nodes| {
            nodes.flatten().any(|node| {
                contains_hint(&node.path().join("name"))
                    || contains_hint(&node.path().join("device/modalias"))
            })
        })
}

fn is_video_node(name: &str) -> bool {
    name.strip_prefix("video").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn v4l2_capture_queue(capabilities: Capabilities) -> Option<QueueType> {
    if !capabilities.contains(Capabilities::STREAMING) {
        return None;
    }
    if capabilities.contains(Capabilities::VIDEO_M2M_MPLANE) {
        Some(QueueType::VideoCaptureMplane)
    } else if capabilities.contains(Capabilities::VIDEO_M2M) {
        Some(QueueType::VideoCapture)
    } else {
        None
    }
}

pub(crate) fn v4l2_encoders() -> (bool, bool) {
    if platform_is_amlogic()
        && !env::var_os("ONE_KVM_V4L2M2M_ALLOW")
            .is_some_and(|value| !value.is_empty() && value != "0")
    {
        log::warn!(
            "V4L2 M2M probing is disabled on Amlogic. Set ONE_KVM_V4L2M2M_ALLOW=1 to enable it."
        );
        return (false, false);
    }

    let Ok(devices) = fs::read_dir("/dev") else {
        return (false, false);
    };

    let (mut h264, mut hevc) = (false, false);
    for device in devices.flatten() {
        if !device.file_name().to_str().is_some_and(is_video_node) {
            continue;
        }
        let Ok(file) = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(device.path())
        else {
            continue;
        };
        let Ok(capability) = querycap::<Capability>(&file) else {
            continue;
        };
        let Some(queue) = v4l2_capture_queue(capability.device_caps()) else {
            continue;
        };

        // From a codec's point of view, CAPTURE is the processed (encoded) output.
        for format in FormatIterator::new(&file, queue) {
            match &format.pixelformat.to_fourcc() {
                b"H264" => h264 = true,
                b"HEVC" => hevc = true,
                _ => {}
            }
        }

        if h264 && hevc {
            break;
        }
    }
    (h264, hevc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_probe_uses_the_capture_side_of_streaming_m2m_nodes() {
        assert_eq!(
            v4l2_capture_queue(Capabilities::VIDEO_CAPTURE | Capabilities::STREAMING),
            None
        );
        assert_eq!(v4l2_capture_queue(Capabilities::VIDEO_M2M), None);
        assert_eq!(
            v4l2_capture_queue(Capabilities::VIDEO_M2M | Capabilities::STREAMING),
            Some(QueueType::VideoCapture)
        );
        assert_eq!(
            v4l2_capture_queue(Capabilities::VIDEO_M2M_MPLANE | Capabilities::STREAMING),
            Some(QueueType::VideoCaptureMplane)
        );
    }
}
