use jni::objects::{JByteBuffer, JString};
use jni::objects::JValue;
use jni::sys::jboolean;
use jni::JNIEnv;
use jni::{
    objects::{GlobalRef, JClass, JObject},
    strings::JNIString,
    JavaVM,
};

use base::message_proto::MultiClipboards;
use hbb_common::protobuf::Message;
use jni::errors::{Error as JniError, Result as JniResult};
use lazy_static::lazy_static;
use std::os::raw::c_void;
use std::sync::{Mutex, RwLock};

lazy_static! {
    static ref JVM: RwLock<Option<JavaVM>> = RwLock::new(None);
    static ref APPLICATION_CONTEXT: RwLock<Option<GlobalRef>> = RwLock::new(None);
    static ref NDK_CONTEXT_INITED: Mutex<bool> = Default::default();
    static ref CLIPBOARD_MANAGER: RwLock<Option<GlobalRef>> = RwLock::new(None);
    static ref MEDIA_CODEC_INFOS: RwLock<Vec<MediaCodecInfo>> = RwLock::new(Vec::new());
    static ref CLIPBOARDS_CLIENT: Mutex<Option<MultiClipboards>> = Mutex::new(None);
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct MediaCodecInfo {
    pub name: String,
    pub mime_type: String,
    pub color_formats: Vec<i32>,
}

pub fn get_decoder_infos(mime: &str) -> Vec<MediaCodecInfo> {
    MEDIA_CODEC_INFOS
        .read()
        .unwrap()
        .iter()
        .filter(|info| info.mime_type == mime)
        .cloned()
        .collect()
}

#[no_mangle]
pub extern "system" fn Java_ffi_FFI_setCodecInfo(mut env: JNIEnv, _class: JClass, info: JString) {
    match env.get_string(&info) {
        Ok(info) => match serde_json::from_str::<Vec<MediaCodecInfo>>(&info.to_string_lossy()) {
            Ok(infos) => *MEDIA_CODEC_INFOS.write().unwrap() = infos,
            Err(error) => log::error!("Invalid Android decoder inventory: {error}"),
        },
        Err(error) => log::error!("Failed to read Android decoder inventory: {error}"),
    };
}

pub fn get_clipboards(client: bool) -> Option<MultiClipboards> {
    if !client {
        return None;
    }
    CLIPBOARDS_CLIENT.lock().ok()?.take()
}

#[no_mangle]
pub extern "system" fn Java_ffi_FFI_onClipboardUpdate(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) {
    if let Ok(data) = env.get_direct_buffer_address(&buffer) {
        if let Ok(len) = env.get_direct_buffer_capacity(&buffer) {
            let data = unsafe { std::slice::from_raw_parts(data, len) };
            if let Some((&1, payload)) = data.split_first() {
                if let Ok(clips) = MultiClipboards::parse_from_bytes(payload) {
                    *CLIPBOARDS_CLIENT.lock().unwrap() = Some(clips);
                }
            }
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_ffi_FFI_setClipboardManager(
    env: JNIEnv,
    _class: JClass,
    clipboard_manager: JObject,
) {
    log::debug!("ClipboardManager init from java");
    if let Ok(jvm) = env.get_java_vm() {
        let mut jvm_lock = JVM.write().unwrap();
        if jvm_lock.is_none() {
            *jvm_lock = Some(jvm);
        }
        drop(jvm_lock);
        if let Ok(manager) = env.new_global_ref(clipboard_manager) {
            *CLIPBOARD_MANAGER.write().unwrap() = Some(manager);
        }
    }
}

fn _call_clipboard_manager<S, T>(name: S, sig: T, args: &[JValue]) -> JniResult<()>
where
    S: Into<JNIString>,
    T: Into<JNIString> + AsRef<str>,
{
    if let (Some(jvm), Some(cm)) = (
        JVM.read().unwrap().as_ref(),
        CLIPBOARD_MANAGER.read().unwrap().as_ref(),
    ) {
        let mut env = jvm.attach_current_thread()?;
        env.call_method(cm, name, sig, args)?;
        return Ok(());
    } else {
        return Err(JniError::ThrowFailed(-1));
    }
}

pub fn call_clipboard_manager_update_clipboard(data: &[u8]) -> JniResult<()> {
    if let (Some(jvm), Some(cm)) = (
        JVM.read().unwrap().as_ref(),
        CLIPBOARD_MANAGER.read().unwrap().as_ref(),
    ) {
        let mut env = jvm.attach_current_thread()?;
        let data = env.byte_array_from_slice(data)?;

        env.call_method(
            cm,
            "rustUpdateClipboard",
            "([B)V",
            &[JValue::Object(&JObject::from(data))],
        )?;
        return Ok(());
    } else {
        return Err(JniError::ThrowFailed(-1));
    }
}

pub fn call_clipboard_manager_enable_client_clipboard(enable: bool) -> JniResult<()> {
    _call_clipboard_manager(
        "rustEnableClientClipboard",
        "(Z)V",
        &[JValue::Bool(jboolean::from(enable))],
    )
}

fn init_ndk_context(java_vm: *mut c_void, context_jobject: *mut c_void) {
    let mut lock = NDK_CONTEXT_INITED.lock().unwrap();
    if *lock {
        unsafe {
            ndk_context::release_android_context();
        }
        *lock = false;
    }
    unsafe {
        ndk_context::initialize_android_context(java_vm, context_jobject);
        #[cfg(feature = "hwcodec")]
        hwcodec::android::ffmpeg_set_java_vm(java_vm);
    }
    *lock = true;
}

fn try_init_rustls_platform_verifier(env: &mut JNIEnv, context_jobject: *mut c_void) {
    use hbb_common::config::ANDROID_RUSTLS_PLATFORM_VERIFIER_INITIALIZED as INITIALIZED;
    use std::sync::atomic::Ordering;
    let initialized = INITIALIZED.load(Ordering::Relaxed);
    if !initialized {
        let ctx_for_rustls = unsafe { JObject::from_raw(context_jobject as jni::sys::jobject) };
        if let Err(e) =
            hbb_common::rustls_platform_verifier::android::init_hosted(env, ctx_for_rustls)
        {
            log::error!("Failed to initialize rustls-platform-verifier: {:?}", e);
        } else {
            INITIALIZED.store(true, Ordering::Relaxed);
            log::info!("rustls-platform-verifier initialized successfully");
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_ffi_FFI_onAppStart(mut env: JNIEnv, _class: JClass, ctx: JObject) {
    if ctx.is_null() {
        log::error!("application context is null");
        return;
    }
    if APPLICATION_CONTEXT.read().unwrap().is_some() {
        log::info!("application context already initialized");
        return;
    }
    if let Ok(jvm) = env.get_java_vm() {
        if let Ok(context) = env.new_global_ref(ctx) {
            let java_vm = jvm.get_java_vm_pointer() as *mut c_void;
            let context_jobject = context.as_obj().as_raw() as *mut c_void;
            *APPLICATION_CONTEXT.write().unwrap() = Some(context);
            *JVM.write().unwrap() = Some(jvm);
            init_ndk_context(java_vm, context_jobject);
            try_init_rustls_platform_verifier(&mut env, context_jobject);
        }
    }
}
