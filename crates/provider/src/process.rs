//! Native owned process capture shared by direct providers and command adapters.
//! The caller supplies command/configuration; this module owns pipes, interruption,
//! process groups, and reaping. It never schedules language nodes.
use crate::{CallContext, ProviderError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct CaptureLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub truncate_stderr: bool,
}

pub struct CapturedStream {
    pub bytes: Vec<u8>,
    pub total_bytes: usize,
    pub truncated: bool,
}

pub struct ProcessOutput {
    pub status: std::process::ExitStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
}

/// Completion includes explicit interruption and reaping of the owned process.
/// All pipes are polled in this future; there are no detached capture tasks.
pub async fn capture(
    command: &mut tokio::process::Command,
    input_bytes: Option<&[u8]>,
    context: &CallContext,
    limits: CaptureLimits,
) -> Result<ProcessOutput, ProviderError> {
    if let Some(cause) = context
        .execution
        .as_ref()
        .and_then(|context| context.cancellation())
    {
        return Err(ProviderError::Cancelled {
            cause,
            cleanup_succeeded: true,
        });
    }
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    if input_bytes.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| ProviderError::ProcessSpawn {
            message: error.to_string(),
        })?;
    let mut process_group = ProcessGroupGuard::new(child.id());
    let timeout = context.effective_timeout();
    let process_id = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let io_error = |error: std::io::Error| ProviderError::ProcessSpawn {
        message: error.to_string(),
    };
    // Own the child outside the interruptible future so cancellation explicitly kills and reaps it.
    let completed = {
        let operation = async {
            let input = async {
                if let (Some(value), Some(mut input)) = (input_bytes, stdin) {
                    input.write_all(value).await.map_err(io_error)?;
                    input.shutdown().await.map_err(io_error)?;
                }
                Ok::<_, ProviderError>(())
            };
            let status = async { child.wait().await.map_err(io_error) };
            tokio::try_join!(
                status,
                bounded_process_output(stdout, limits.stdout_bytes, false),
                bounded_process_output(stderr, limits.stderr_bytes, limits.truncate_stderr),
                input
            )
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            cause = context.cancelled() => Err(ProviderError::Cancelled { cause, cleanup_succeeded: false }),
            result = &mut operation => result,
            _ = tokio::time::sleep(timeout) => Err(ProviderError::ProcessTimeout {
                timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                cleanup_succeeded: false,
            }),
        }
    };
    let (status, stdout, stderr, ()) = match completed {
        Ok(output) => {
            // A completed leader may leave background descendants with closed
            // pipes. Their ownership ends with this call too.
            #[cfg(unix)]
            if !cleanup_process_group(process_id).await {
                return Err(ProviderError::ProcessCleanup { primary: None });
            }
            process_group.disarm();
            output
        }
        Err(mut error) => {
            let _ = child.start_kill();
            let (group_clean, reaped) = tokio::join!(
                cleanup_process_group(process_id),
                tokio::time::timeout(Duration::from_secs(1), child.wait())
            );
            let cleanup_succeeded = group_clean && matches!(reaped, Ok(Ok(_)));
            if cleanup_succeeded {
                process_group.disarm();
            }
            match &mut error {
                ProviderError::Cancelled {
                    cleanup_succeeded: value,
                    ..
                }
                | ProviderError::ProcessTimeout {
                    cleanup_succeeded: value,
                    ..
                } => *value = cleanup_succeeded,
                _ if !cleanup_succeeded => {
                    return Err(ProviderError::ProcessCleanup {
                        primary: Some(Box::new(error)),
                    });
                }
                _ => {}
            }
            return Err(error);
        }
    };

    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

async fn bounded_process_output<R: tokio::io::AsyncRead + Unpin>(
    stream: Option<R>,
    limit: usize,
    truncate: bool,
) -> Result<CapturedStream, ProviderError> {
    let mut bytes = Vec::new();
    let mut total_bytes = 0usize;
    if let Some(mut stream) = stream {
        let mut chunk = [0u8; 8192];
        loop {
            let read =
                stream
                    .read(&mut chunk)
                    .await
                    .map_err(|error| ProviderError::ProcessSpawn {
                        message: error.to_string(),
                    })?;
            if read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(read);
            if !truncate && total_bytes > limit {
                return Err(ProviderError::ProcessOutputTooLarge { limit });
            }
            bytes.extend_from_slice(&chunk[..read.min(limit.saturating_sub(bytes.len()))]);
        }
    }
    Ok(CapturedStream {
        truncated: total_bytes > limit,
        bytes,
        total_bytes,
    })
}

struct ProcessGroupGuard {
    process_id: Option<u32>,
}

impl ProcessGroupGuard {
    const fn new(process_id: Option<u32>) -> Self {
        Self { process_id }
    }

    fn disarm(&mut self) {
        self.process_id = None;
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let Some(process_id) = self.process_id.and_then(|id| i32::try_from(id).ok()) else {
            return;
        };
        // The provider created this process group. A synchronous signal in Drop
        // keeps cancellation of the parent future from leaving descendants alive.
        unsafe {
            libc::kill(-process_id, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {}
}

#[cfg(unix)]
async fn cleanup_process_group(process_id: Option<u32>) -> bool {
    let Some(process_id) = process_id.and_then(|id| i32::try_from(id).ok()) else {
        return false;
    };
    // A just-exited, unreaped group leader can transiently reject a signal
    // (including EPERM on macOS). Reaping runs concurrently; verify disappearance
    // after it completes instead of misclassifying that race as failed cleanup.
    let mut signalled = false;
    for _ in 0..50 {
        if !signalled {
            signalled = unsafe { libc::kill(-process_id, libc::SIGKILL) } == 0;
            if !signalled && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
        }
        if unsafe { libc::kill(-process_id, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[cfg(not(unix))]
async fn cleanup_process_group(_process_id: Option<u32>) -> bool {
    false
}
