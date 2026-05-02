use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;

pub fn from_str<T>(contents: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    catch_parser_panic(|| serde_yml::from_str(contents).map_err(Into::into))
}

fn catch_parser_panic<T, F>(parse: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    static PANIC_HOOK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    let hook_guard = PANIC_HOOK_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let result = panic::catch_unwind(AssertUnwindSafe(parse));

    panic::set_hook(previous_hook);
    drop(hook_guard);

    match result {
        Ok(result) => result,
        Err(payload) => Err(anyhow!(
            "YAML parser panicked{}",
            format_panic_payload(&payload)
        )),
    }
}

fn format_panic_payload(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        format!(": {message}")
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        format!(": {message}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_parser_panic_converts_panic_to_error() {
        let err = catch_parser_panic::<(), _>(|| Err(anyhow!("boom"))).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn catch_parser_panic_recovers_from_actual_panic() {
        let err = catch_parser_panic::<(), _>(|| panic!("boom")).unwrap_err();
        assert!(err.to_string().contains("YAML parser panicked: boom"));
    }
}
