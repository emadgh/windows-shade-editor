use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub fn catch_value<T, F>(label: &str, task: F) -> Result<T, String>
where
    F: FnOnce() -> T,
{
    match catch_unwind(AssertUnwindSafe(task)) {
        Ok(value) => Ok(value),
        Err(payload) => Err(format!("{label} panicked: {}", panic_payload_text(payload.as_ref()))),
    }
}

pub fn catch_result<T, F>(label: &str, task: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    match catch_value(label, task) {
        Ok(result) => result,
        Err(err) => Err(err),
    }
}

fn panic_payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caught_worker_panic_becomes_an_error() {
        let result = catch_result("Export worker", || -> Result<(), String> {
            panic!("synthetic worker panic")
        });
        let err = result.unwrap_err();
        assert!(err.contains("Export worker panicked"));
        assert!(err.contains("synthetic worker panic"));
    }
}
