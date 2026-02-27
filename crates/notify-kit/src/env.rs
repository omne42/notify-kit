use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::{
    FeishuWebhookConfig, FeishuWebhookSink, GenericWebhookConfig, GenericWebhookSink, Hub,
    HubConfig, Sink, SlackWebhookConfig, SlackWebhookSink, SoundConfig, SoundSink,
};

#[derive(Debug, Clone, Copy)]
pub struct StandardEnvHubOptions {
    pub default_sound_enabled: bool,
    pub require_sink: bool,
}

impl Default for StandardEnvHubOptions {
    fn default() -> Self {
        Self {
            default_sound_enabled: false,
            require_sink: false,
        }
    }
}

fn parse_bool_env_value(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn env_bool(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_bool_env_value(&value))
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_timeout_ms_env(key: &str) -> anyhow::Result<Duration> {
    let timeout = env_nonempty(key)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(5000);
    Ok(Duration::from_millis(timeout.max(1)))
}

pub fn build_hub_from_standard_env(options: StandardEnvHubOptions) -> anyhow::Result<Option<Hub>> {
    const OMNE_NOTIFY_SOUND_ENV: &str = "OMNE_NOTIFY_SOUND";
    const OMNE_NOTIFY_WEBHOOK_URL_ENV: &str = "OMNE_NOTIFY_WEBHOOK_URL";
    const OMNE_NOTIFY_WEBHOOK_FIELD_ENV: &str = "OMNE_NOTIFY_WEBHOOK_FIELD";
    const OMNE_NOTIFY_FEISHU_WEBHOOK_URL_ENV: &str = "OMNE_NOTIFY_FEISHU_WEBHOOK_URL";
    const OMNE_NOTIFY_SLACK_WEBHOOK_URL_ENV: &str = "OMNE_NOTIFY_SLACK_WEBHOOK_URL";
    const OMNE_NOTIFY_TIMEOUT_MS_ENV: &str = "OMNE_NOTIFY_TIMEOUT_MS";
    const OMNE_NOTIFY_EVENTS_ENV: &str = "OMNE_NOTIFY_EVENTS";

    let sound_enabled = env_bool(OMNE_NOTIFY_SOUND_ENV).unwrap_or(options.default_sound_enabled);
    let timeout = parse_timeout_ms_env(OMNE_NOTIFY_TIMEOUT_MS_ENV)
        .with_context(|| format!("invalid {OMNE_NOTIFY_TIMEOUT_MS_ENV}"))?;

    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
    if sound_enabled {
        sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
    }

    if let Some(url) = env_nonempty(OMNE_NOTIFY_WEBHOOK_URL_ENV) {
        let mut cfg = GenericWebhookConfig::new(url).with_timeout(timeout);
        if let Some(field) = env_nonempty(OMNE_NOTIFY_WEBHOOK_FIELD_ENV) {
            cfg = cfg.with_payload_field(field);
        }
        sinks.push(Arc::new(
            GenericWebhookSink::new(cfg).context("build generic webhook sink")?,
        ));
    }

    if let Some(url) = env_nonempty(OMNE_NOTIFY_FEISHU_WEBHOOK_URL_ENV) {
        let cfg = FeishuWebhookConfig::new(url).with_timeout(timeout);
        sinks.push(Arc::new(
            FeishuWebhookSink::new(cfg).context("build feishu sink")?,
        ));
    }

    if let Some(url) = env_nonempty(OMNE_NOTIFY_SLACK_WEBHOOK_URL_ENV) {
        let cfg = SlackWebhookConfig::new(url).with_timeout(timeout);
        sinks.push(Arc::new(
            SlackWebhookSink::new(cfg).context("build slack sink")?,
        ));
    }

    if sinks.is_empty() {
        if options.require_sink {
            anyhow::bail!(
                "no notification sinks configured (enable {OMNE_NOTIFY_SOUND_ENV}=1 or provide webhook envs)"
            );
        }
        return Ok(None);
    }

    let enabled_kinds = std::env::var(OMNE_NOTIFY_EVENTS_ENV).ok().and_then(|raw| {
        let set = raw
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        if set.is_empty() { None } else { Some(set) }
    });

    Ok(Some(Hub::new(
        HubConfig {
            enabled_kinds,
            per_sink_timeout: timeout,
        },
        sinks,
    )))
}
