//! User settings, saved between runs (eframe storage).
//!
//! "Modules" are optional parts of the client. A module that's switched off
//! doesn't run at all: no work each frame and nothing loaded into memory.

use serde::{Deserialize, Serialize};

use crate::theme::ThemeChoice;

pub const CONFIG_KEY: &str = "config_v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeChoice,
    /// UI scale (1.0 = 100%).
    pub zoom: f32,
    pub modules: Modules,

    /// Keep the token on this PC so you don't have to log in each time.
    pub remember_login: bool,
    /// Saved token (only if `remember_login`). Prefixed "Bot " for bot accounts.
    pub saved_token: Option<String>,

    /// Messages kept in memory per channel. Older ones are dropped.
    pub max_messages_per_channel: usize,
    /// How many channels keep their messages cached. Leaving more channels than
    /// this frees the oldest one (and its images).
    pub cached_channels: usize,
    /// Ask before opening links in the browser.
    pub warn_before_links: bool,

    /// Pop up a desktop notification for mentions / DMs (Notifications module).
    pub desktop_notifications: bool,
    /// Flash the taskbar entry when a notification arrives and the window
    /// isn't focused.
    pub flash_taskbar: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            zoom: 1.0,
            modules: Modules::default(),
            remember_login: true,
            saved_token: None,
            max_messages_per_channel: 200,
            cached_channels: 8,
            warn_before_links: true,
            desktop_notifications: true,
            flash_taskbar: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    /// Move/resize panels.
    pub layout_editor: bool,
    /// Avatars, server icons and image attachments. Off = text only, lowest RAM.
    pub images: bool,
    /// Colour emoji as images (Twemoji + custom server emoji).
    pub emoji_images: bool,
    /// "X is typing…", both showing and sending.
    pub typing_indicators: bool,
    /// Member list panel contents.
    pub member_list: bool,
    /// Desktop notifications, taskbar flash and the 🔔 mentions inbox.
    pub notifications: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            layout_editor: true,
            images: true,
            emoji_images: true,
            typing_indicators: true,
            member_list: true,
            notifications: true,
        }
    }
}

/// Description of each module for the Modules settings page.
pub struct ModuleInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub get: fn(&mut Modules) -> &mut bool,
}

pub const MODULES: &[ModuleInfo] = &[
    ModuleInfo {
        name: "Layout editor",
        description: "Move and resize panels. When off, the layout is locked.",
        get: |m| &mut m.layout_editor,
    },
    ModuleInfo {
        name: "Images",
        description: "Avatars, server icons and image attachments. Turn off for the lowest memory use.",
        get: |m| &mut m.images,
    },
    ModuleInfo {
        name: "Emoji images",
        description: "Show emoji in colour like Discord (Twemoji and custom server emoji).",
        get: |m| &mut m.emoji_images,
    },
    ModuleInfo {
        name: "Typing indicators",
        description: "Show when others are typing, and let them see when you are.",
        get: |m| &mut m.typing_indicators,
    },
    ModuleInfo {
        name: "Member list",
        description: "The panel listing people in the current DM or recently active in a channel.",
        get: |m| &mut m.member_list,
    },
    ModuleInfo {
        name: "Notifications",
        description: "Desktop notifications and taskbar flashing for mentions and DMs, plus the 🔔 mentions inbox. Unread markers work either way.",
        get: |m| &mut m.notifications,
    },
];
