use hbb_common::{anyhow::Context, log, ResultType};
use std::os::windows::ffi::OsStrExt;
use windows::{
    core::{Interface, BSTR},
    Win32::{
        Foundation::{RPC_E_CHANGED_MODE, VARIANT_FALSE},
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED,
            },
            RemoteDesktop::ProcessIdToSessionId,
            TaskScheduler::{
                IExecAction, ITaskService, TaskScheduler, TASK_ACTION_EXEC, TASK_CREATE,
                TASK_LOGON_SERVICE_ACCOUNT, TASK_RUN_USE_SESSION_ID,
            },
            Variant::VARIANT,
        },
    },
};

pub(super) fn start(is_setup: bool, shared_memory_name: Option<&str>) -> ResultType<()> {
    let exe = std::env::current_exe()?;
    let directory = exe
        .parent()
        .context("Portable service path has no parent")?;
    let mut arguments = String::new();
    if is_setup {
        arguments.push_str("--noinstall ");
    }
    arguments.push_str("--run-as-system");
    if let Some(name) = shared_memory_name {
        arguments.push(' ');
        arguments.push_str(&crate::portable_service::portable_service_shmem_arg(name));
    }
    let mut session = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }?;
    let session = i32::try_from(session)?;

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let _apartment = if initialized == RPC_E_CHANGED_MODE {
        None
    } else {
        initialized.ok()?;
        Some(Apartment)
    };

    let task_name = BSTR::from(format!("XSCC-PortableService-{}", uuid::Uuid::new_v4()));
    let empty = VARIANT::default();
    let system = VARIANT::from("SYSTEM");
    let task_access = VARIANT::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    unsafe {
        let scheduler: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        scheduler.Connect(&empty, &empty, &empty, &empty)?;
        let folder = scheduler.GetFolder(&BSTR::from("\\"))?;

        let definition = scheduler.NewTask(0)?;
        let settings = definition.Settings()?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_FALSE)?;
        settings.SetStopIfGoingOnBatteries(VARIANT_FALSE)?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from_wide(
            &exe.as_os_str().encode_wide().collect::<Vec<_>>(),
        ))?;
        action.SetWorkingDirectory(&BSTR::from_wide(
            &directory.as_os_str().encode_wide().collect::<Vec<_>>(),
        ))?;
        action.SetArguments(&BSTR::from(arguments))?;

        let task = folder.RegisterTaskDefinition(
            &task_name,
            &definition,
            TASK_CREATE.0,
            &system,
            &empty,
            TASK_LOGON_SERVICE_ACCOUNT,
            &task_access,
        )?;
        let started = task.RunEx(&empty, TASK_RUN_USE_SESSION_ID.0, session, &BSTR::new());
        if let Err(error) = folder.DeleteTask(&task_name, 0) {
            log::warn!("Failed to remove on-demand portable service task: {error}");
        }
        started.context("Failed to start the portable service in the current session")?;
    }
    Ok(())
}
