use super::ffi::{set_decoder_infos, MediaCodecInfo};
use hbb_common::ResultType;
use jni::{
    objects::{JIntArray, JObjectArray, JString, JValue},
    JNIEnv, JavaVM,
};
use std::{ffi::c_void, sync::atomic::Ordering};

/// # Safety
/// `java_vm` must be the live JavaVM supplied by the Android application.
pub unsafe fn initialize_android_decoders(java_vm: *mut c_void) -> ResultType<()> {
    let vm = unsafe { JavaVM::from_raw(java_vm.cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let infos = match env.with_local_frame(16, discover) {
        Ok(infos) => infos,
        Err(error) => return Err(with_java_exception(&mut env, error)),
    };
    set_decoder_infos(infos);
    crate::mediacodec::check_mediacodec();
    log::info!(
        "Android hardware NV12 decoder availability: h264={}, h265={}",
        crate::mediacodec::H264_DECODER_SUPPORT.load(Ordering::SeqCst),
        crate::mediacodec::H265_DECODER_SUPPORT.load(Ordering::SeqCst),
    );
    Ok(())
}

fn discover(env: &mut JNIEnv<'_>) -> ResultType<Vec<MediaCodecInfo>> {
    // REGULAR_CODECS excludes codecs requiring special secure/tunneled setup.
    let list = env.new_object("android/media/MediaCodecList", "(I)V", &[JValue::Int(0)])?;
    let codecs = JObjectArray::from(
        env.call_method(
            list,
            "getCodecInfos",
            "()[Landroid/media/MediaCodecInfo;",
            &[],
        )?
        .l()?,
    );
    let mut infos = Vec::new();
    for index in 0..env.get_array_length(&codecs)? {
        let mut codec_infos =
            env.with_local_frame(24, |env| -> ResultType<Vec<MediaCodecInfo>> {
                let codec = env.get_object_array_element(&codecs, index)?;
                if env.call_method(&codec, "isEncoder", "()Z", &[])?.z()?
                    || !env
                        .call_method(&codec, "isHardwareAccelerated", "()Z", &[])?
                        .z()?
                {
                    return Ok(Vec::new());
                }
                let name = JString::from(
                    env.call_method(&codec, "getName", "()Ljava/lang/String;", &[])?
                        .l()?,
                );
                let name: String = env.get_string(&name)?.into();
                let types = JObjectArray::from(
                    env.call_method(&codec, "getSupportedTypes", "()[Ljava/lang/String;", &[])?
                        .l()?,
                );
                let mut infos = Vec::new();
                for index in 0..env.get_array_length(&types)? {
                    let info =
                        env.with_local_frame(16, |env| -> ResultType<Option<MediaCodecInfo>> {
                            let mime = JString::from(env.get_object_array_element(&types, index)?);
                            let mime_type: String = env.get_string(&mime)?.into();
                            if !matches!(mime_type.as_str(), "video/avc" | "video/hevc") {
                                return Ok(None);
                            }
                            let capabilities = env
                        .call_method(
                            &codec,
                            "getCapabilitiesForType",
                            "(Ljava/lang/String;)Landroid/media/MediaCodecInfo$CodecCapabilities;",
                            &[JValue::Object(mime.as_ref())],
                        )?
                        .l()?;
                            let formats = JIntArray::from(
                                env.get_field(capabilities, "colorFormats", "[I")?.l()?,
                            );
                            let mut color_formats =
                                vec![0; env.get_array_length(&formats)? as usize];
                            env.get_int_array_region(&formats, 0, &mut color_formats)?;
                            // The existing decoder can read NV12, not flexible/vendor YUV layouts.
                            Ok(color_formats.contains(&21).then(|| MediaCodecInfo {
                                name: name.clone(),
                                mime_type,
                                color_formats,
                            }))
                        })?;
                    if let Some(info) = info {
                        infos.push(info);
                    }
                }
                Ok(infos)
            })?;
        infos.append(&mut codec_infos);
    }
    Ok(infos)
}

fn with_java_exception(
    env: &mut JNIEnv<'_>,
    error: hbb_common::anyhow::Error,
) -> hbb_common::anyhow::Error {
    let exception = match env.exception_occurred() {
        Ok(exception) => exception,
        Err(cause) => return error.context(format!("Cannot inspect Java exception: {cause}")),
    };
    if exception.is_null() {
        return error;
    }
    if let Err(cause) = env.exception_clear() {
        return error.context(format!("Cannot clear Java exception: {cause}"));
    }
    let description = env.with_local_frame(8, |env| -> ResultType<String> {
        let text = JString::from(
            env.call_method(exception, "toString", "()Ljava/lang/String;", &[])?
                .l()?,
        );
        let description: String = env.get_string(&text)?.into();
        Ok(description)
    });
    match description {
        Ok(description) => error.context(description),
        Err(cause) => {
            let cleanup = env.exception_clear();
            error.context(format!(
                "Cannot describe Java exception: {cause}; cleanup: {cleanup:?}"
            ))
        }
    }
}
