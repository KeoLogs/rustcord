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
    /// Extra domains whose links open without asking (and their subdomains).
    pub trusted_domains: Vec<String>,

    /// Show messages as typed: no bold, quotes, code blocks... (mentions,
    /// emoji and links still work).
    pub disable_formatting: bool,
    /// 3:05 PM instead of 15:05.
    pub use_12_hour_time: bool,
    /// Load pictures in embeds / link previews (Embeds module).
    pub show_embed_images: bool,
    /// New replies ping the author by default (the reply banner's @ ON/OFF).
    pub mention_on_reply: bool,
    /// Show messages from blocked users instead of "N blocked messages".
    pub show_blocked_messages: bool,
    /// Compact message display (name inline, no avatars). Synced with
    /// Discord's setting for user accounts.
    pub compact_messages: bool,
    /// Which platform the gateway IDENTIFY claims (user accounts; next
    /// connect). Web unless the user opts in.
    pub client_identity: ClientIdentity,

    /// Pop up a desktop notification for mentions / DMs (Notifications module).
    pub desktop_notifications: bool,
    /// Flash the taskbar entry when a notification arrives and the window
    /// isn't focused.
    pub flash_taskbar: bool,

    /// Smaller member list rows without the activity line.
    pub compact_member_list: bool,
    /// Bot accounts: ask for the Server Members + Presence intents and load
    /// the whole member list. Both must be enabled in the Developer Portal.
    pub bot_member_intents: bool,
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
            trusted_domains: Vec::new(),
            disable_formatting: false,
            use_12_hour_time: false,
            show_embed_images: true,
            mention_on_reply: true,
            show_blocked_messages: false,
            compact_messages: false,
            client_identity: ClientIdentity::Web,
            desktop_notifications: true,
            flash_taskbar: true,
            compact_member_list: false,
            bot_member_intents: false,
        }
    }
}

/// "Show as" (Settings → Privacy & Safety): the IDENTIFY `properties`, which
/// decide the platform icon friends see next to your status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientIdentity {
    /// Honest: browser "RustCord" (shows as web / browser).
    #[default]
    Web,
    /// Claims to be Discord's desktop app on this OS.
    Desktop,
    /// Claims to be Discord's Android app.
    Mobile,
}

impl ClientIdentity {
    pub const ALL: [ClientIdentity; 3] = [
        ClientIdentity::Web,
        ClientIdentity::Desktop,
        ClientIdentity::Mobile,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ClientIdentity::Web => "Web (RustCord)",
            ClientIdentity::Desktop => "Desktop",
            ClientIdentity::Mobile => "Mobile",
        }
    }

    /// IDENTIFY `d.properties`. `os` is `std::env::consts::OS`.
    pub fn properties(self, os: &str) -> serde_json::Value {
        use serde_json::json;
        match self {
            ClientIdentity::Web => {
                json!({ "os": os, "browser": "RustCord", "device": "RustCord" })
            }
            ClientIdentity::Desktop => {
                let os = match os {
                    "windows" => "Windows",
                    "macos" => "Mac OS X",
                    _ => "Linux",
                };
                json!({ "os": os, "browser": "Discord Client", "device": "" })
            }
            ClientIdentity::Mobile => {
                json!({ "os": "Android", "browser": "Discord Android", "device": "" })
            }
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
    /// Member list panel contents, online status and profile popouts' status.
    pub member_list: bool,
    /// Desktop notifications, taskbar flash and the 🔔 mentions inbox.
    pub notifications: bool,
    /// Thin bar at the bottom: connection state, channel, typing, RAM.
    pub status_bar: bool,
    /// Embeds and link previews under messages.
    pub embeds: bool,
    /// `@` / `#` / `:` suggestions in the message box.
    pub autocomplete: bool,
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
            status_bar: false,
            embeds: true,
            autocomplete: true,
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
        name: "Member list & presence",
        description: "The member list (grouped by role, with online status and activities) and status dots. Off = no member list requests and no presence kept.",
        get: |m| &mut m.member_list,
    },
    ModuleInfo {
        name: "Notifications",
        description: "Desktop notifications and taskbar flashing for mentions and DMs, plus the 🔔 mentions inbox. Unread markers work either way.",
        get: |m| &mut m.notifications,
    },
    ModuleInfo {
        name: "Status bar",
        description: "A thin bar at the bottom of the window (like Discord Messenger's) with the connection state, the open channel, who's typing and RAM use.",
        get: |m| &mut m.status_bar,
    },
    ModuleInfo {
        name: "Embeds & link previews",
        description: "Cards under messages for links (YouTube, articles...) and bot embeds. Off = embeds aren't kept or drawn.",
        get: |m| &mut m.embeds,
    },
    ModuleInfo {
        name: "Autocomplete",
        description: "Suggestions while typing @mentions, #channels and :emoji: in the message box.",
        get: |m| &mut m.autocomplete,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_properties() {
        let web = ClientIdentity::Web.properties("linux");
        assert_eq!(web["browser"], "RustCord");
        assert_eq!(web["os"], "linux");
        assert_eq!(web["device"], "RustCord");
        let desk = ClientIdentity::Desktop.properties("windows");
        assert_eq!(desk["browser"], "Discord Client");
        assert_eq!(desk["os"], "Windows");
        assert_eq!(desk["device"], "");
        assert_eq!(
            ClientIdentity::Desktop.properties("macos")["os"],
            "Mac OS X"
        );
        assert_eq!(ClientIdentity::Desktop.properties("linux")["os"], "Linux");
        let mobile = ClientIdentity::Mobile.properties("linux");
        assert_eq!(mobile["browser"], "Discord Android");
        assert_eq!(mobile["os"], "Android");
        // Old configs without the field stay on Web.
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.client_identity, ClientIdentity::Web);
    }
}
