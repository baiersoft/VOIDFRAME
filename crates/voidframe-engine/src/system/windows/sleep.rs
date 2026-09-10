//! `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)`
//! is per-thread, so it lives on one dedicated OS thread that stays alive
//! for the process. `on = true` asserts, `on = false` clears
//! (`ES_CONTINUOUS` alone). The OS clears it when the process exits, so
//! nothing needs journaling.

use crate::error::{Error, Result};
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};

type Request = (bool, Sender<Result<()>>);

static CONTROL: OnceLock<Sender<Request>> = OnceLock::new();

fn control() -> &'static Sender<Request> {
    CONTROL.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        std::thread::Builder::new()
            .name("voidframe-sleep-inhibit".into())
            .spawn(move || {
                for (on, reply) in rx {
                    let flags = if on {
                        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
                    } else {
                        ES_CONTINUOUS
                    };
                    // SAFETY: pure flag call on this thread; a zero return means failure.
                    let prev = unsafe { SetThreadExecutionState(flags) };
                    let _ = reply.send(if prev.0 == 0 {
                        Err(Error::msg("SetThreadExecutionState returned 0".into()))
                    } else {
                        Ok(())
                    });
                }
            })
            .expect("spawn sleep-inhibit thread");
        tx
    })
}

pub async fn inhibit(on: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let (reply_tx, reply_rx) = channel();
        control()
            .send((on, reply_tx))
            .map_err(|_| Error::msg("sleep-inhibit thread is gone".into()))?;
        reply_rx
            .recv()
            .map_err(|_| Error::msg("sleep-inhibit thread dropped the reply".into()))?
    })
    .await
    .map_err(|e| Error::msg(format!("inhibit_sleep panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inhibit_on_then_off_round_trips_on_the_dedicated_thread() {
        inhibit(true).await.unwrap();
        inhibit(false).await.unwrap();
    }
}
