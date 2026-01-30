use std::io::Write;

use crate::Event;
use crate::event::Severity;
use crate::sinks::{BoxFuture, Sink};

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

    fn send_command(command_argv: &[String]) -> anyhow::Result<()> {
        let (program, args) = command_argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("sound command argv is empty"))?;

        let _child = std::process::Command::new(program)
            .args(args)
            .spawn()
            .map_err(|err| anyhow::anyhow!("spawn sound command {program}: {err}"))?;
        Ok(())
    }
}

impl Sink for SoundSink {
    fn name(&self) -> &'static str {
        "sound"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if let Some(argv) = self.command_argv.as_deref() {
                Self::send_command(argv)?;
                return Ok(());
            }

            Self::send_terminal_bell(event)?;
            Ok(())
        })
    }
}
