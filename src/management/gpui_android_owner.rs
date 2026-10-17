use std::sync::{Arc, Mutex, Weak};
use xscc_native_management::manager::NativeManager;

// Activities own their manager. The session adapter must not keep its file lock
// alive after the Activity and its runtime have released their strong references.
static OWNER: Mutex<Weak<NativeManager>> = Mutex::new(Weak::new());

pub(super) fn attach(manager: &Arc<NativeManager>) -> Result<(), &'static str> {
    let mut owner = OWNER.lock().unwrap();
    if owner.upgrade().is_some() {
        return Err("Native management owner is already initialized");
    }
    *owner = Arc::downgrade(manager);
    Ok(())
}

pub(super) fn get() -> Result<Arc<NativeManager>, &'static str> {
    OWNER
        .lock()
        .unwrap()
        .upgrade()
        .ok_or("Native management owner is unavailable")
}
