use voidframe_engine::model::module::Hive;
use voidframe_engine::system::{RegKey, RegValue, SystemController};

const CPU_NAME_SUBKEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
const CPU_NAME_VALUE: &str = "ProcessorNameString";

pub(crate) async fn cpu_model_impl(sys: &dyn SystemController) -> Result<String, String> {
    let key = RegKey {
        hive: Hive::Hklm,
        subkey: CPU_NAME_SUBKEY.into(),
        value_name: CPU_NAME_VALUE.into(),
    };
    match sys.read_registry(&key).await.map_err(|e| e.to_string())? {
        RegValue::Sz(name) => Ok(name.trim().to_string()),
        RegValue::Absent => Err(format!(
            "registry value {CPU_NAME_SUBKEY}\\{CPU_NAME_VALUE} is absent"
        )),
        other => Err(format!(
            "registry value {CPU_NAME_SUBKEY}\\{CPU_NAME_VALUE} was not a string: {other:?}"
        )),
    }
}

#[specta::specta]
#[tauri::command]
pub async fn cpu_model(state: tauri::State<'_, crate::state::AppState>) -> Result<String, String> {
    cpu_model_impl(state.sys.as_ref()).await
}

pub(crate) async fn gpu_model_impl(sys: &dyn SystemController) -> Result<String, String> {
    sys.gpu_model().await.map_err(|e| e.to_string())
}

#[specta::specta]
#[tauri::command]
pub async fn gpu_model(state: tauri::State<'_, crate::state::AppState>) -> Result<String, String> {
    gpu_model_impl(state.sys.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    #[tokio::test]
    async fn cpu_model_reads_the_processor_name_string_value() {
        let sys = MockController::new().with_registry(
            voidframe_engine::model::module::Hive::Hklm,
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
            voidframe_engine::system::RegValue::Sz("AMD Ryzen 7 9800X3D 8-Core Processor".into()),
        );
        let model = cpu_model_impl(&sys).await.unwrap();
        assert_eq!(model, "AMD Ryzen 7 9800X3D 8-Core Processor");
    }

    #[tokio::test]
    async fn cpu_model_errors_with_a_clear_message_when_the_value_is_absent() {
        let sys = MockController::new();
        let err = cpu_model_impl(&sys).await.unwrap_err();
        assert!(err.contains("ProcessorNameString"), "error was: {err}");
    }

    #[tokio::test]
    async fn gpu_model_reads_the_controller_reported_gpu_name() {
        let sys = MockController::new().with_gpu_model("NVIDIA GeForce RTX 4090");
        let model = gpu_model_impl(&sys).await.unwrap();
        assert_eq!(model, "NVIDIA GeForce RTX 4090");
    }
}
