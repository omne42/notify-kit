use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::config::HubConfig;
use crate::event::{Event, EventKind};
use crate::sinks::{FeishuWebhookSink, Sink, SoundSink};

#[derive(Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
}

struct HubInner {
    enabled_kinds: BTreeSet<EventKind>,
    sinks: Vec<Arc<dyn Sink>>,
    per_sink_timeout: Duration,
}

#[derive(Debug)]
pub struct HubInitError(anyhow::Error);

impl std::fmt::Display for HubInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "init notify hub: {}", self.0)
    }
}

impl std::error::Error for HubInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl From<anyhow::Error> for HubInitError {
    fn from(value: anyhow::Error) -> Self {
        Self(value)
    }
}

impl Hub {
    pub fn from_env() -> Result<Option<Self>, HubInitError> {
        let Some(config) = HubConfig::from_env().map_err(HubInitError::from)? else {
            return Ok(None);
        };
        let hub = Self::new(config)?;
        Ok(Some(hub))
    }

    fn new(config: HubConfig) -> Result<Self, HubInitError> {
        let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();

        if let Some(sound) = config.sound.clone() {
            sinks.push(Arc::new(SoundSink::new(sound)));
        }
        if let Some(feishu) = config.feishu.clone() {
            sinks.push(Arc::new(FeishuWebhookSink::new(feishu)?));
        }

        let inner = HubInner {
            enabled_kinds: config.enabled_kinds,
            sinks,
            // Notifications must never block the main app; keep this aggressive.
            per_sink_timeout: Duration::from_secs(2),
        };
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub fn notify(&self, event: Event) {
        if !self.inner.enabled_kinds.contains(&event.kind) {
            return;
        }

        let inner = self.inner.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                sink = "hub",
                "notify dropped (no tokio runtime): kind={:?} title={:?}",
                event.kind,
                event.title
            );
            return;
        };

        handle.spawn(async move {
            if let Err(err) = inner.send(event).await {
                tracing::warn!(sink = "hub", "notify failed: {err:#}");
            }
        });
    }
}

impl HubInner {
    async fn send(self: Arc<Self>, event: Event) -> anyhow::Result<()> {
        let mut join_set = tokio::task::JoinSet::<(String, anyhow::Result<()>)>::new();

        for sink in &self.sinks {
            let sink = sink.clone();
            let event = event.clone();
            let timeout = self.per_sink_timeout;
            join_set.spawn(async move {
                let name = sink.name().to_string();
                let res = match tokio::time::timeout(timeout, sink.send(&event)).await {
                    Ok(inner) => inner,
                    Err(_) => Err(anyhow::anyhow!("timeout after {timeout:?}")),
                };
                (name, res)
            });
        }

        let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((_name, Ok(()))) => {}
                Ok((name, Err(err))) => failures.push((name, err)),
                Err(err) => failures.push(("join".to_string(), err.into())),
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        let mut msg = String::from("one or more sinks failed:");
        for (name, err) in failures {
            msg.push_str(&format!("\n- {name}: {err:#}"));
        }
        Err(anyhow::anyhow!(msg))
    }
}
