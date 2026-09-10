//! Task Scheduler 2.0 COM (`ITaskService`) — registers/removes the two
//! M3 tasks. Runs on a dedicated STA-initialised blocking thread per call
//! (the same discipline as `deelevate.rs`): COM objects never cross an
//! `.await`.

use crate::error::{Error, Result};
use crate::system::{TaskPrincipal, TaskSpec, TaskTrigger};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{
    IBootTrigger, IExecAction, ILogonTrigger, ITaskFolder, ITaskService, TASK_ACTION_EXEC,
    TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN, TASK_LOGON_SERVICE_ACCOUNT,
    TASK_RUNLEVEL_HIGHEST, TASK_TRIGGER_BOOT, TASK_TRIGGER_LOGON, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

struct ComApartment;
impl ComApartment {
    fn enter() -> Result<Self> {
        // SAFETY: called once per blocking thread; paired with
        // CoUninitialize in Drop on the same thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| Error::msg(format!("CoInitializeEx: {e}")))?;
        Ok(ComApartment)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `enter`.
        unsafe { CoUninitialize() };
    }
}

fn root_folder() -> Result<(ITaskService, ITaskFolder)> {
    // SAFETY: CLSID/IID from the windows crate; COM is initialised on this thread.
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| Error::msg(format!("CoCreateInstance(TaskScheduler): {e}")))?;
    // SAFETY: empty VARIANTs select the local machine and current credentials.
    unsafe {
        service.Connect(
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
        )
    }
    .map_err(|e| Error::msg(format!("ITaskService::Connect: {e}")))?;
    // SAFETY: `\` is the documented root folder path.
    let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }
        .map_err(|e| Error::msg(format!("ITaskService::GetFolder: {e}")))?;
    Ok((service, folder))
}

fn register_sync(spec: &TaskSpec) -> Result<()> {
    let _apt = ComApartment::enter()?;
    let (service, folder) = root_folder()?;
    // SAFETY: all calls below are on live COM interfaces obtained above;
    // BSTR arguments live until each call returns.
    unsafe {
        let def = service
            .NewTask(0)
            .map_err(|e| Error::msg(format!("NewTask: {e}")))?;
        def.RegistrationInfo()
            .and_then(|ri| ri.SetDescription(&BSTR::from(spec.description.as_str())))
            .map_err(|e| Error::msg(format!("SetDescription: {e}")))?;
        let settings = def
            .Settings()
            .map_err(|e| Error::msg(format!("Settings: {e}")))?;
        settings
            .SetDisallowStartIfOnBatteries(false.into())
            .map_err(|e| Error::msg(format!("SetDisallowStartIfOnBatteries: {e}")))?;
        settings
            .SetStopIfGoingOnBatteries(false.into())
            .map_err(|e| Error::msg(format!("SetStopIfGoingOnBatteries: {e}")))?;
        settings
            .SetStartWhenAvailable(true.into())
            .map_err(|e| Error::msg(format!("SetStartWhenAvailable: {e}")))?;
        settings
            .SetExecutionTimeLimit(&BSTR::from("PT0S")) // no time limit
            .map_err(|e| Error::msg(format!("SetExecutionTimeLimit: {e}")))?;

        let triggers = def
            .Triggers()
            .map_err(|e| Error::msg(format!("Triggers: {e}")))?;
        match &spec.trigger {
            TaskTrigger::AtLogonOfCurrentUser => {
                let t = triggers
                    .Create(TASK_TRIGGER_LOGON)
                    .map_err(|e| Error::msg(format!("Create(logon): {e}")))?;
                let logon: ILogonTrigger = t
                    .cast()
                    .map_err(|e| Error::msg(format!("cast ILogonTrigger: {e}")))?;
                logon
                    .SetId(&BSTR::from("logon"))
                    .map_err(|e| Error::msg(format!("SetId(logon): {e}")))?;
                // UserId left empty = the registering user's logon.
            }
            TaskTrigger::AtBootDelayed { minutes } => {
                let t = triggers
                    .Create(TASK_TRIGGER_BOOT)
                    .map_err(|e| Error::msg(format!("Create(boot): {e}")))?;
                let boot: IBootTrigger = t
                    .cast()
                    .map_err(|e| Error::msg(format!("cast IBootTrigger: {e}")))?;
                boot.SetId(&BSTR::from("boot"))
                    .map_err(|e| Error::msg(format!("SetId(boot): {e}")))?;
                boot.SetDelay(&BSTR::from(format!("PT{minutes}M")))
                    .map_err(|e| Error::msg(format!("SetDelay: {e}")))?;
            }
        }

        let actions = def
            .Actions()
            .map_err(|e| Error::msg(format!("Actions: {e}")))?;
        let action = actions
            .Create(TASK_ACTION_EXEC)
            .map_err(|e| Error::msg(format!("Create(exec): {e}")))?;
        let exec: IExecAction = action
            .cast()
            .map_err(|e| Error::msg(format!("cast IExecAction: {e}")))?;
        exec.SetPath(&BSTR::from(spec.exe.to_string_lossy().as_ref()))
            .map_err(|e| Error::msg(format!("SetPath: {e}")))?;
        exec.SetArguments(&BSTR::from(spec.args.as_str()))
            .map_err(|e| Error::msg(format!("SetArguments: {e}")))?;

        let principal = def
            .Principal()
            .map_err(|e| Error::msg(format!("Principal: {e}")))?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_HIGHEST)
            .map_err(|e| Error::msg(format!("SetRunLevel: {e}")))?;
        let (user, logon_type) = match spec.principal {
            TaskPrincipal::CurrentUserHighest => (VARIANT::default(), TASK_LOGON_INTERACTIVE_TOKEN),
            TaskPrincipal::LocalSystem => (
                VARIANT::from(BSTR::from("SYSTEM")),
                TASK_LOGON_SERVICE_ACCOUNT,
            ),
        };
        folder
            .RegisterTaskDefinition(
                &BSTR::from(spec.name.as_str()),
                &def,
                TASK_CREATE_OR_UPDATE.0,
                &user,
                &VARIANT::default(),
                logon_type,
                &VARIANT::default(),
            )
            .map_err(|e| Error::msg(format!("RegisterTaskDefinition({}): {e}", spec.name)))?;
    }
    Ok(())
}

fn deregister_sync(name: &str) -> Result<()> {
    let _apt = ComApartment::enter()?;
    let (_service, folder) = root_folder()?;
    // SAFETY: live folder interface; BSTR alive for the call.
    match unsafe { folder.DeleteTask(&BSTR::from(name), 0) } {
        Ok(()) => Ok(()),
        // HRESULT 0x80070002 (ERROR_FILE_NOT_FOUND): nothing to delete.
        Err(e) if e.code().0 as u32 == 0x8007_0002 => Ok(()),
        Err(e) => Err(Error::msg(format!("DeleteTask({name}): {e}"))),
    }
}

fn exists_sync(name: &str) -> Result<bool> {
    let _apt = ComApartment::enter()?;
    let (_service, folder) = root_folder()?;
    // SAFETY: live folder interface; BSTR alive for the call.
    match unsafe { folder.GetTask(&BSTR::from(name)) } {
        Ok(_) => Ok(true),
        Err(e) if e.code().0 as u32 == 0x8007_0002 => Ok(false),
        Err(e) => Err(Error::msg(format!("GetTask({name}): {e}"))),
    }
}

pub async fn register(spec: &TaskSpec) -> Result<()> {
    let spec = spec.clone();
    tokio::task::spawn_blocking(move || register_sync(&spec))
        .await
        .map_err(|e| Error::msg(format!("register_task panicked: {e}")))?
}

pub async fn deregister(name: &str) -> Result<()> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || deregister_sync(&name))
        .await
        .map_err(|e| Error::msg(format!("deregister_task panicked: {e}")))?
}

pub async fn exists(name: &str) -> Result<bool> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || exists_sync(&name))
        .await
        .map_err(|e| Error::msg(format!("task_exists panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "registers and removes a real scheduled task -- run manually: `cargo test -p voidframe-engine --lib system::windows::scheduled_task::tests::register_exists_deregister_round_trips -- --ignored --exact`"]
    async fn register_exists_deregister_round_trips() {
        let spec = TaskSpec {
            name: "VOIDFRAME_TestTask".into(),
            description: "voidframe test task".into(),
            trigger: TaskTrigger::AtBootDelayed { minutes: 10 },
            principal: TaskPrincipal::CurrentUserHighest,
            exe: std::env::current_exe().unwrap(),
            args: "--version".into(),
        };
        register(&spec).await.unwrap();
        assert!(exists(&spec.name).await.unwrap());
        deregister(&spec.name).await.unwrap();
        assert!(!exists(&spec.name).await.unwrap());
        deregister(&spec.name).await.unwrap(); // idempotent
    }

    #[tokio::test]
    async fn exists_is_false_for_a_name_that_was_never_registered() {
        assert!(!exists("VOIDFRAME_NeverRegistered_9f3a").await.unwrap());
    }
}
