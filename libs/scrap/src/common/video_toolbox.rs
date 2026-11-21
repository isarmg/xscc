use super::{annex_b, CodecFormat};
use base::message_proto::EncodedVideoFrames;
use hbb_common::{
    anyhow::{anyhow, ensure},
    bail, log, ResultType,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMBlockBufferAssureMemoryNowFlag, kCMTimeInvalid, kCMVideoCodecType_H264,
    kCMVideoCodecType_HEVC, CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo,
    CMTime, CMVideoCodecType, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionCreateFromHEVCParameterSets,
};
use objc2_core_video::{
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
};
use objc2_video_toolbox::{
    kVTDecompressionPropertyKey_RealTime,
    kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder, VTDecodeFrameFlags,
    VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord, VTDecompressionSession,
    VTIsHardwareDecodeSupported, VTSessionSetProperty,
};
use std::{
    convert::TryFrom,
    ffi::c_void,
    ptr::{self, NonNull},
    sync::Mutex,
};

#[derive(Debug)]
pub struct Surface(CFRetained<CVPixelBuffer>);

// A decoder publishes a retained, read-only IOSurface. CoreVideo can recycle
// its backing only after the final owner (including the GPU submission) releases it.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    pub fn extent(&self) -> (u32, u32) {
        (
            CVPixelBufferGetWidth(&self.0) as u32,
            CVPixelBufferGetHeight(&self.0) as u32,
        )
    }

    pub fn into_pixel_buffer(self) -> CFRetained<CVPixelBuffer> {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
enum Codec {
    H264,
    Hevc,
}

impl Codec {
    fn from_format(format: CodecFormat) -> Option<Self> {
        match format {
            CodecFormat::H264 => Some(Self::H264),
            CodecFormat::H265 => Some(Self::Hevc),
            _ => None,
        }
    }

    fn id(self) -> CMVideoCodecType {
        match self {
            Self::H264 => kCMVideoCodecType_H264,
            Self::Hevc => kCMVideoCodecType_HEVC,
        }
    }

    fn parameter_set(self, kind: u8) -> Option<usize> {
        match (self, kind) {
            (Self::H264, 7 | 8) => Some(usize::from(kind - 7)),
            (Self::Hevc, 32..=34) => Some(usize::from(kind - 32)),
            _ => None,
        }
    }

    fn unit_kind(self, nal: &[u8]) -> ResultType<u8> {
        ensure!(nal[0] & 0x80 == 0, "Invalid H.26x NAL header");
        match self {
            Self::H264 => Ok(nal[0] & 31),
            Self::Hevc if nal.len() >= 2 => Ok((nal[0] >> 1) & 63),
            Self::Hevc => bail!("Incomplete HEVC NAL header"),
        }
    }

    fn picture(self, kind: u8) -> bool {
        match self {
            Self::H264 => (1..=5).contains(&kind),
            Self::Hevc => kind <= 31,
        }
    }

    fn format_description(
        self,
        parameters: &[Vec<u8>; 3],
    ) -> ResultType<CFRetained<CMFormatDescription>> {
        let count = match self {
            Self::H264 => 2,
            Self::Hevc => 3,
        };
        ensure!(
            parameters[..count].iter().all(|set| !set.is_empty()),
            "Missing VideoToolbox parameter sets"
        );
        let mut pointers = parameters
            .each_ref()
            .map(|set| NonNull::from(set.as_slice()).cast());
        let mut sizes = parameters.each_ref().map(|set| set.len());
        let mut format = ptr::null();
        // The framework copies the parameter sets into the format description.
        let result = unsafe {
            let pointers = NonNull::from(&mut pointers[0]);
            let sizes = NonNull::from(&mut sizes[0]);
            let output = NonNull::from(&mut format);
            match self {
                Self::H264 => CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    None, count, pointers, sizes, 4, output,
                ),
                Self::Hevc => CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    None, count, pointers, sizes, 4, None, output,
                ),
            }
        };
        owned(
            result,
            format.cast_mut(),
            "Create VideoToolbox video format",
        )
    }
}

pub(super) fn supported(format: CodecFormat) -> bool {
    Codec::from_format(format)
        .is_some_and(|codec| unsafe { VTIsHardwareDecodeSupported(codec.id()) })
}

pub(super) struct Decoder {
    codec: Codec,
    parameters: [Vec<u8>; 3],
    sample: Vec<u8>,
    session: Option<Session>,
    output: Option<Surface>,
}

impl Decoder {
    pub(super) fn new(format: CodecFormat) -> Option<Self> {
        let codec = Codec::from_format(format)?;
        supported(format).then(|| Self {
            codec,
            parameters: Default::default(),
            sample: Vec::new(),
            session: None,
            output: None,
        })
    }

    pub(super) fn decode(&mut self, frames: &EncodedVideoFrames) -> ResultType<bool> {
        self.output = None;
        for picture in &frames.frames {
            self.sample.clear();
            let mut has_picture = false;
            let mut format_changed = false;
            let units = annex_b::units(&picture.data)
                .ok_or_else(|| anyhow!("Missing H.26x Annex B start code"))?;
            for nal in units {
                let kind = self.codec.unit_kind(nal)?;
                if let Some(index) = self.codec.parameter_set(kind) {
                    if self.parameters[index] != nal {
                        self.parameters[index].clear();
                        self.parameters[index].extend_from_slice(nal);
                        format_changed = true;
                    }
                    continue;
                }
                self.sample
                    .extend_from_slice(&u32::try_from(nal.len())?.to_be_bytes());
                self.sample.extend_from_slice(nal);
                has_picture |= self.codec.picture(kind);
            }
            if format_changed {
                self.session = None;
            }
            if !has_picture {
                continue;
            }
            if self.session.is_none() {
                self.session = Some(Session::new(
                    self.codec.format_description(&self.parameters)?,
                )?);
            }
            let session = self
                .session
                .as_ref()
                .expect("VideoToolbox session was initialized");
            if let Some(surface) = session.decode(&self.sample, picture.pts)? {
                self.output = Some(surface);
            }
        }
        Ok(self.output.is_some())
    }

    pub(super) fn take_surface(&mut self) -> Option<Surface> {
        self.output.take()
    }
}

struct Session {
    decoder: CFRetained<VTDecompressionSession>,
    format: CFRetained<CMFormatDescription>,
}

impl Session {
    fn new(format: CFRetained<CMFormatDescription>) -> ResultType<Self> {
        let pixel_format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32);
        let io_surface = CFDictionary::<CFType, CFType>::empty();
        let (attributes, specification) = unsafe {
            let attributes = CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[&pixel_format, &io_surface, CFBoolean::new(true)],
            );
            let specification = CFDictionary::<CFString, CFBoolean>::from_slices(
                &[kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder],
                &[CFBoolean::new(true)],
            );
            (attributes, specification)
        };
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decoded),
            decompressionOutputRefCon: ptr::null_mut(),
        };
        let mut decoder = ptr::null_mut();
        let result = unsafe {
            VTDecompressionSession::create(
                None,
                &format,
                Some(specification.as_opaque()),
                Some(attributes.as_opaque()),
                &callback,
                NonNull::from(&mut decoder),
            )
        };
        let decoder = owned(result, decoder, "Create hardware VideoToolbox session")?;
        let result = unsafe {
            VTSessionSetProperty(
                &decoder,
                kVTDecompressionPropertyKey_RealTime,
                Some(CFBoolean::new(true)),
            )
        };
        if result != 0 {
            log::trace!("VideoToolbox real-time option returned OSStatus {result}");
        }
        Ok(Self { decoder, format })
    }

    fn decode(&self, data: &[u8], pts: i64) -> ResultType<Option<Surface>> {
        let mut block = ptr::null_mut();
        let length = data.len();
        let result = unsafe {
            CMBlockBuffer::create_with_memory_block(
                None,
                ptr::null_mut(),
                length,
                None,
                ptr::null(),
                0,
                length,
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block),
            )
        };
        let block = owned(result, block, "Allocate VideoToolbox input")?;
        let result = unsafe {
            CMBlockBuffer::replace_data_bytes(NonNull::from(data).cast(), &block, 0, length)
        };
        check(result, "Copy VideoToolbox input")?;
        let mut sample = ptr::null_mut();
        let timing = unsafe {
            CMSampleTimingInfo {
                duration: kCMTimeInvalid,
                presentationTimeStamp: CMTime::new(pts, 1000),
                decodeTimeStamp: kCMTimeInvalid,
            }
        };
        let result = unsafe {
            CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(&self.format),
                1,
                1,
                &timing,
                1,
                &length,
                NonNull::from(&mut sample),
            )
        };
        let sample = owned(result, sample, "Create VideoToolbox input sample")?;
        let output = Mutex::new((0, None::<Surface>));
        // With both asynchronous and temporal processing flags clear,
        // VideoToolbox calls back before returning. The callback can run on a decoder thread.
        let result = unsafe {
            self.decoder.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                NonNull::from(&output).as_ptr().cast(),
                ptr::null_mut(),
            )
        };
        check(result, "Submit VideoToolbox sample")?;
        let (status, surface) = output
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        check(status, "Decode VideoToolbox sample")?;
        if let Some(surface) = &surface {
            let (width, height) = surface.extent();
            ensure!(
                width > 0
                    && height > 0
                    && CVPixelBufferGetPixelFormatType(&surface.0)
                        == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
                    && CVPixelBufferGetPlaneCount(&surface.0) == 2
                    && CVPixelBufferGetIOSurface(Some(&surface.0)).is_some(),
                "VideoToolbox returned a frame incompatible with the Metal surface renderer"
            );
        }
        Ok(surface)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { self.decoder.invalidate() };
    }
}

unsafe extern "C-unwind" fn decoded(
    _: *mut c_void,
    frame: *mut c_void,
    status: i32,
    _: VTDecodeInfoFlags,
    image: *mut CVImageBuffer,
    _: CMTime,
    _: CMTime,
) {
    // The input call owns this context until the synchronous callback completes.
    let Some(output) = (unsafe { frame.cast::<Mutex<(i32, Option<Surface>)>>().as_ref() }) else {
        return;
    };
    let mut result = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    result.0 = status;
    result.1 = NonNull::new(image).map(|image| Surface(unsafe { CFRetained::retain(image) }));
}

fn check(status: i32, operation: &str) -> ResultType<()> {
    ensure!(status == 0, "{operation} failed (OSStatus {status})");
    Ok(())
}

fn owned<T: objc2_core_foundation::Type>(
    status: i32,
    value: *mut T,
    operation: &str,
) -> ResultType<CFRetained<T>> {
    check(status, operation)?;
    let ptr = NonNull::new(value).ok_or_else(|| anyhow!("{operation} returned no object"))?;
    // Every caller passes the output of a CoreFoundation Create function.
    Ok(unsafe { CFRetained::from_raw(ptr) })
}
