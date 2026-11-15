//! Typed capture/encoder restart reasons, independent of the transport.
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoRestart {
    Refresh,
    NewSubscriber,
    CodecChanged,
    ChromaChanged,
    QualityChanged,
    FrameRateChanged,
    EncoderReconfiguration,
    DisplayChanged,
    PrivacyChanged,
    DesktopChanged,
    PortableChanged,
    ScaleChanged,
    CaptureLost,
    CaptureBackendChanged,
    Screenshot,
}

impl VideoRestart {
    #[cfg(any(windows, test))]
    pub(super) fn retry_after_platform_update(
        self,
        desktop_changed: impl FnOnce() -> bool,
    ) -> bool {
        // OpenInputDesktop/SetThreadDesktop failures leave the old thread desktop selected.
        self != Self::DesktopChanged || !desktop_changed()
    }

    pub(super) fn reuse_capture(self) -> bool {
        matches!(
            self,
            Self::Refresh
                | Self::NewSubscriber
                | Self::CodecChanged
                | Self::ChromaChanged
                | Self::QualityChanged
                | Self::FrameRateChanged
                | Self::EncoderReconfiguration
        )
    }
}

impl fmt::Display for VideoRestart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "video reconfiguration: {self:?}")
    }
}

impl std::error::Error for VideoRestart {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_encoder_reconfiguration_reuses_capture() {
        assert!(VideoRestart::NewSubscriber.reuse_capture());
        assert!(VideoRestart::FrameRateChanged.reuse_capture());
        for reason in [
            VideoRestart::DisplayChanged,
            VideoRestart::PrivacyChanged,
            VideoRestart::CaptureLost,
        ] {
            assert!(!reason.reuse_capture());
        }
    }

    #[test]
    fn an_unavailable_input_desktop_keeps_using_error_backoff_until_selection_succeeds() {
        // The Windows API may repeatedly deny opening the secure desktop without changing state.
        for _ in 0..3 {
            assert!(!VideoRestart::DesktopChanged.retry_after_platform_update(|| true));
        }
        assert!(VideoRestart::DesktopChanged.retry_after_platform_update(|| false));
        assert!(VideoRestart::Refresh.retry_after_platform_update(|| panic!("unrelated restart")));
    }
}
