//! Local, explicit paste for foreground native form fields. Never poll clipboard
//! availability, coerce content URIs, or forward these contents to a remote peer.
use jni::{
    Env, JValue,
    errors::Result,
    jni_sig, jni_str,
    objects::{JObject, JString},
};

pub(crate) fn read_text(env: &mut Env<'_>, activity: &JObject<'_>) -> Result<Option<String>> {
    let service = env.new_string("clipboard")?;
    let manager = env
        .call_method(
            activity,
            jni_str!("getSystemService"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
            &[JValue::Object(&service)],
        )?
        .l()?;
    if manager.is_null() {
        return Ok(None);
    }
    let clip = env
        .call_method(
            &manager,
            jni_str!("getPrimaryClip"),
            jni_sig!("()Landroid/content/ClipData;"),
            &[],
        )?
        .l()?;
    if clip.is_null()
        || env
            .call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?
            .i()?
            == 0
    {
        return Ok(None);
    }
    let item = env
        .call_method(
            &clip,
            jni_str!("getItemAt"),
            jni_sig!("(I)Landroid/content/ClipData$Item;"),
            &[JValue::Int(0)],
        )?
        .l()?;
    // getText does not resolve a URI or ask another application for data.
    let text = env
        .call_method(
            &item,
            jni_str!("getText"),
            jni_sig!("()Ljava/lang/CharSequence;"),
            &[],
        )?
        .l()?;
    if text.is_null() {
        return Ok(None);
    }
    let text = env
        .call_method(
            &text,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    let text = env.cast_local::<JString>(text)?;
    Ok(Some(text.try_to_string(env)?))
}
