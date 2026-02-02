#![forbid(unsafe_code)]

mod event;
mod hub;
mod sinks;

pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, TryNotifyError};
pub use crate::sinks::{
    BarkConfig, BarkSink, DingTalkWebhookConfig, DingTalkWebhookSink, DiscordWebhookConfig,
    DiscordWebhookSink, FeishuWebhookConfig, FeishuWebhookSink, GenericWebhookConfig,
    GenericWebhookSink, GitHubCommentConfig, GitHubCommentSink, PushPlusConfig, PushPlusSink,
    ServerChanConfig, ServerChanSink, Sink, SlackWebhookConfig, SlackWebhookSink, SoundConfig,
    SoundSink, TelegramBotConfig, TelegramBotSink, WeComWebhookConfig, WeComWebhookSink,
};
