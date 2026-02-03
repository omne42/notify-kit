use std::io::Write;
#[cfg(not(feature = "sound-command"))]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Event;
use crate::event::Severity;
use crate::sinks::{BoxFuture, Sink};

#[cfg(not(feature = "sound-command"))]
static WARNED_SOUND_COMMAND_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct SoundConfig {
    pub command_argv: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct SoundSink {
    command_argv: Option<Vec<String>>,
}

impl SoundSink {
    pub fn new(config: SoundConfig) -> Self {
        Self {
            command_argv: config.command_argv,
        }
    }

    fn bell_count(severity: Severity) -> usize {
        match severity {
            Severity::Error => 2,
            Severity::Warning => 1,
            Severity::Info | Severity::Success => 1,
        }
    }

    fn send_terminal_bell(event: &Event) -> anyhow::Result<()> {
        let bell = "\u{0007}";
        let count = Self::bell_count(event.severity);
        let mut stderr = std::io::stderr().lock();
        for _ in 0..count {
            stderr.write_all(bell.as_bytes())?;
        }
        stderr.flush()?;
        Ok(())
    }

    #[cfg(feature = "sound-command")]
    fn send_command(command_argv: &[String]) -> anyhow::Result<()> {
        let (program, args) = command_argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("sound command argv is empty"))?;

        if program.trim().is_empty() {
            return Err(anyhow::anyhow!("sound command program is empty"));
        }

        let mut child = std::process::Command::new(program)
            .args(args)
            .spawn()
            .map_err(|err| anyhow::anyhow!("spawn sound command {program}: {err}"))?;

        let program = program.to_string();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(move || match child.wait() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    tracing::warn!(
                        sink = "sound",
                        program = %program,
                        status = ?status,
                        "sound command exited non-zero"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        sink = "sound",
                        program = %program,
                        "wait sound command failed: {err}"
                    );
                }
            });
        } else {
            match child.wait() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    tracing::warn!(
                        sink = "sound",
                        program = %program,
                        status = ?status,
                        "sound command exited non-zero"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        sink = "sound",
                        program = %program,
                        "wait sound command failed: {err}"
                    );
                }
            }
        }
        Ok(())
    }
}

impl Sink for SoundSink {
    fn name(&self) -> &'static str {
        "sound"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if let Some(_argv) = self.command_argv.as_deref() {
                #[cfg(feature = "sound-command")]
                {
                    Self::send_command(_argv)?;
                    return Ok(());
                }

                #[cfg(not(feature = "sound-command"))]
                {
                    if !WARNED_SOUND_COMMAND_DISABLED.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            sink = "sound",
                            "sound command_argv configured but feature \"sound-command\" is disabled; falling back to terminal bell"
                        );
                    }
                    Self::send_terminal_bell(event)?;
                    return Ok(());
                }
            }

            Self::send_terminal_bell(event)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sound-command")]
    use super::*;

    #[cfg(feature = "sound-command")]
    #[test]
    fn send_command_rejects_empty_argv() {
        let err = SoundSink::send_command(&[]).expect_err("expected error");
        assert!(err.to_string().contains("argv is empty"), "{err:#}");
    }

    #[cfg(feature = "sound-command")]
    #[test]
    fn send_command_rejects_empty_program() {
        let err = SoundSink::send_command(&[String::from("  ")]).expect_err("expected error");
        assert!(err.to_string().contains("program is empty"), "{err:#}");
    }
}
