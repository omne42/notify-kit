use std::collections::BTreeSet;

use anyhow::Context;

use crate::event::EventKind;

#[derive(Debug, Clone)]
pub struct SoundConfig {
    pub command_argv: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct FeishuConfig {
    pub webhook_url: String,
}

#[derive(Debug, Clone)]
pub struct HubConfig {
    pub sound: Option<SoundConfig>,
    pub(crate) feishu: Option<FeishuConfig>,
    pub enabled_kinds: BTreeSet<EventKind>,
}

impl HubConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let sound = if parse_env_bool("CODE_PM_NOTIFY_SOUND")? {
            Some(SoundConfig {
                command_argv: parse_env_json_string_array("CODE_PM_NOTIFY_SOUND_CMD_JSON")?,
            })
        } else {
            None
        };

        let feishu = std::env::var("CODE_PM_NOTIFY_FEISHU_WEBHOOK_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(|webhook_url| FeishuConfig { webhook_url });

        let enabled_kinds = parse_env_event_kinds("CODE_PM_NOTIFY_EVENTS")?;

        if sound.is_none() && feishu.is_none() {
            return Ok(None);
        }

        Ok(Some(Self {
            sound,
            feishu,
            enabled_kinds,
        }))
    }
}

fn parse_env_bool(key: &str) -> anyhow::Result<bool> {
    let Some(value) = std::env::var(key).ok() else {
        return Ok(false);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(false);
    }
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "no" | "n" | "off" => Ok(false),
        other => anyhow::bail!("{key}: invalid boolean value: {other}"),
    }
}

fn parse_env_json_string_array(key: &str) -> anyhow::Result<Option<Vec<String>>> {
    let Some(raw) = std::env::var(key).ok() else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let values = serde_json::from_str::<Vec<String>>(raw)
        .with_context(|| format!("{key}: parse json string array"))?;
    let values = values
        .into_iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(values))
}

fn parse_env_event_kinds(key: &str) -> anyhow::Result<BTreeSet<EventKind>> {
    let Some(raw) = std::env::var(key).ok() else {
        return Ok(BTreeSet::from([
            EventKind::TurnCompleted,
            EventKind::ApprovalRequested,
        ]));
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(BTreeSet::from([
            EventKind::TurnCompleted,
            EventKind::ApprovalRequested,
        ]));
    }

    let mut out = BTreeSet::<EventKind>::new();
    for part in raw.split(',') {
        let value = part.trim().to_ascii_lowercase();
        if value.is_empty() {
            continue;
        }
        match value.as_str() {
            "turn_completed" => {
                out.insert(EventKind::TurnCompleted);
            }
            "approval_requested" => {
                out.insert(EventKind::ApprovalRequested);
            }
            "message_received" => {
                out.insert(EventKind::MessageReceived);
            }
            other => anyhow::bail!(
                "{key}: unknown event kind: {other} (expected: turn_completed, approval_requested, message_received)"
            ),
        }
    }

    if out.is_empty() {
        anyhow::bail!("{key}: must include at least one event kind");
    }

    Ok(out)
}
