//! User settings, saved between runs (eframe storage).
//!
//! "Modules" are optional parts of the client. A module that's switched off
//! doesn't run at all: no work each frame and nothing loaded into memory.

use serde::{Deserialize, Serialize};

use crate::theme::ThemeChoice;

pub const CONFIG_KEY: &str = "config_v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeChoice,
    /// Id (folder name) of the applied custom theme, if any; it overrides
    /// `theme` while the Custom themes module is on.
    #[serde(default)]
    pub custom_theme: Option<String>,
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
    /// Emoji picker: most recently used emoji (newest first).
    #[serde(default)]
    pub recent_emoji: crate::emoji_picker::RecentEmoji,
    /// Emoji picker skin tone (0 = default, 1-5 light to dark).
    #[serde(default)]
    pub skin_tone: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            custom_theme: None,
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
            recent_emoji: Default::default(),
            skin_tone: 0,
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
    ///
    /// Discord picks the platform icon friends see (desktop / mobile / web)
    /// from these. Desktop and Mobile copy what the official apps send
    /// (versions are plausible, not exact); Web is RustCord's honest
    /// identity. A change needs a fresh IDENTIFY (not a RESUME), so saving
    /// it reconnects; other open sessions (the official app, a browser tab)
    /// can still decide which icon friends see.
    pub fn properties(self, os: &str) -> serde_json::Value {
        use serde_json::json;
        match self {
            ClientIdentity::Web => {
                json!({ "os": os, "browser": "RustCord", "device": "RustCord" })
            }
            ClientIdentity::Desktop => {
                let (os, os_version) = match os {
                    "windows" => ("Windows", "10.0.22631"),
                    "macos" => ("Mac OS X", "23.6.0"),
                    _ => ("Linux", "6.8.0"),
                };
                json!({
                    "os": os,
                    "browser": "Discord Client",
                    "device": "",
                    "release_channel": "stable",
                    "client_version": DESKTOP_CLIENT_VERSION,
                    "os_version": os_version,
                    "os_arch": "x64",
                    "app_arch": "x64",
                    "system_locale": "en-US",
                    "has_client_mods": false,
                })
            }
            ClientIdentity::Mobile => json!({
                "os": "Android",
                "browser": "Discord Android",
                "device": "Pixel 8",
                "release_channel": "googleRelease",
                "client_version": MOBILE_CLIENT_VERSION,
                "os_version": "34",
                "os_arch": "arm64-v8a",
                "system_locale": "en-US",
            }),
        }
    }
}

/// Plausible current versions of the official apps for "Show as".
const DESKTOP_CLIENT_VERSION: &str = "1.0.9215";
const MOBILE_CLIENT_VERSION: &str = "300.18 - rn";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Desktop notifications, taskbar flash and the mentions inbox.
    pub notifications: bool,
    /// Thin bar at the bottom: connection state, channel, typing, RAM.
    pub status_bar: bool,
    /// Embeds and link previews under messages.
    pub embeds: bool,
    /// `@` / `#` / `:` suggestions in the message box.
    pub autocomplete: bool,
    /// GIF button + picker in the message box (Discord's GIF search).
    pub gif_picker: bool,
    /// Reactions under messages + the add-reaction picker.
    #[serde(default = "on")]
    pub reactions: bool,
    /// Ctrl+F message search.
    #[serde(default = "on")]
    pub search: bool,
    /// Friends / Message Requests pages and the Home header.
    #[serde(default = "on")]
    pub friends: bool,
    /// Threads, forum channels and the thread browser.
    #[serde(default = "on")]
    pub threads: bool,
    /// Invite cards, Join a Server, rules screening and onboarding.
    #[serde(default = "on")]
    pub invites: bool,
    /// Settings, Layout: preset picker and layout copy / import.
    #[serde(default = "on")]
    pub layout_presets: bool,
    /// Themes imported from .rctheme files.
    #[serde(default = "on")]
    pub custom_themes: bool,
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
            gif_picker: true,
            reactions: true,
            search: true,
            friends: true,
            threads: true,
            invites: true,
            layout_presets: true,
            custom_themes: true,
        }
    }
}

/// serde default for module flags added after a release (default on).
fn on() -> bool {
    true
}

/// Module flags readable off the UI thread (the gateway task), so a module
/// that is off does no gateway work. The app stores them every frame.
pub mod live {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering::Relaxed;

    pub static REACTIONS: AtomicBool = AtomicBool::new(true);
    pub static SEARCH: AtomicBool = AtomicBool::new(true);
    pub static FRIENDS: AtomicBool = AtomicBool::new(true);
    pub static THREADS: AtomicBool = AtomicBool::new(true);
    pub static INVITES: AtomicBool = AtomicBool::new(true);
    pub static LAYOUT_PRESETS: AtomicBool = AtomicBool::new(true);
    pub static CUSTOM_THEMES: AtomicBool = AtomicBool::new(true);
    // (end of flags)

    pub fn sync(m: &super::Modules) {
        REACTIONS.store(m.reactions, Relaxed);
        SEARCH.store(m.search, Relaxed);
        FRIENDS.store(m.friends, Relaxed);
        THREADS.store(m.threads, Relaxed);
        INVITES.store(m.invites, Relaxed);
        LAYOUT_PRESETS.store(m.layout_presets, Relaxed);
        CUSTOM_THEMES.store(m.custom_themes, Relaxed);
    }

    pub fn on(flag: &AtomicBool) -> bool {
        flag.load(Relaxed)
    }
}

/// Group a module is listed under on the Modules settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleCategory {
    Messaging,
    Social,
    Servers,
    Appearance,
}

impl ModuleCategory {
    /// In page order.
    pub const ALL: [ModuleCategory; 4] = [
        ModuleCategory::Messaging,
        ModuleCategory::Social,
        ModuleCategory::Servers,
        ModuleCategory::Appearance,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ModuleCategory::Messaging => "Messaging",
            ModuleCategory::Social => "Social",
            ModuleCategory::Servers => "Servers",
            ModuleCategory::Appearance => "Appearance",
        }
    }
}

/// Description of each module for the Modules settings page.
pub struct ModuleInfo {
    pub name: &'static str,
    pub category: ModuleCategory,
    pub description: &'static str,
    pub get: fn(&mut Modules) -> &mut bool,
}

pub const MODULES: &[ModuleInfo] = &[
    ModuleInfo {
        name: "Layout editor",
        category: ModuleCategory::Appearance,
        description: "Move and resize panels. When off, the layout is locked.",
        get: |m| &mut m.layout_editor,
    },
    ModuleInfo {
        name: "Images",
        category: ModuleCategory::Appearance,
        description: "Avatars, server icons and image attachments. Turn off for the lowest memory use.",
        get: |m| &mut m.images,
    },
    ModuleInfo {
        name: "Emoji images",
        category: ModuleCategory::Appearance,
        description: "Show emoji in colour like Discord (Twemoji and custom server emoji).",
        get: |m| &mut m.emoji_images,
    },
    ModuleInfo {
        name: "Typing indicators",
        category: ModuleCategory::Messaging,
        description: "Show when others are typing, and let them see when you are.",
        get: |m| &mut m.typing_indicators,
    },
    ModuleInfo {
        name: "Member list & presence",
        category: ModuleCategory::Social,
        description: "The member list (grouped by role, with online status and activities) and status dots. Off = no member list requests and no presence kept.",
        get: |m| &mut m.member_list,
    },
    ModuleInfo {
        name: "Notifications",
        category: ModuleCategory::Social,
        description: "Desktop notifications and taskbar flashing for mentions and DMs, plus the mentions inbox (bell in the chat header). Unread markers work either way.",
        get: |m| &mut m.notifications,
    },
    ModuleInfo {
        name: "Status bar",
        category: ModuleCategory::Appearance,
        description: "A thin bar at the bottom of the window (like Discord Messenger's) with the connection state, the open channel, who's typing and RAM use.",
        get: |m| &mut m.status_bar,
    },
    ModuleInfo {
        name: "Embeds & link previews",
        category: ModuleCategory::Messaging,
        description: "Cards under messages for links (YouTube, articles...) and bot embeds. Off = embeds aren't kept or drawn.",
        get: |m| &mut m.embeds,
    },
    ModuleInfo {
        name: "Autocomplete",
        category: ModuleCategory::Messaging,
        description: "Suggestions while typing @mentions, #channels and :emoji: in the message box.",
        get: |m| &mut m.autocomplete,
    },
    ModuleInfo {
        name: "GIF picker",
        category: ModuleCategory::Messaging,
        description: "The GIF button in the message box: trending GIFs and Tenor search through Discord (user accounts). Off = no button, nothing loaded.",
        get: |m| &mut m.gif_picker,
    },
    ModuleInfo {
        name: "Reactions",
        category: ModuleCategory::Messaging,
        description: "Reaction pills under messages, the add-reaction button and the emoji picker. Off = no reactions shown or loaded.",
        get: |m| &mut m.reactions,
    },
    ModuleInfo {
        name: "Search",
        category: ModuleCategory::Messaging,
        description: "Ctrl+F message search (this channel, or the whole server / DM). Off = no search box, nothing loaded.",
        get: |m| &mut m.search,
    },
    ModuleInfo {
        name: "Friends & message requests",
        category: ModuleCategory::Social,
        description: "Friends and Message Requests pages and the Home search header above your DMs (user accounts). Off = plain DM list, nothing loaded.",
        get: |m| &mut m.friends,
    },
    ModuleInfo {
        name: "Threads & forums",
        category: ModuleCategory::Servers,
        description: "Threads under their channels, forum channels with posts, the Threads browser and Create Thread. Off = threads and forums hidden, nothing loaded.",
        get: |m| &mut m.threads,
    },
    ModuleInfo {
        name: "Invites & onboarding",
        category: ModuleCategory::Servers,
        description: "Invite cards under messages, the rail's Add a Server button, server rules screening and onboarding questions. Off = invite links stay plain links, nothing loaded.",
        get: |m| &mut m.invites,
    },
    ModuleInfo {
        name: "Layout presets",
        category: ModuleCategory::Appearance,
        description: "Ready-made panel layouts and copying / importing a layout as text in Settings, Layout. Off = only Edit layout and Reset.",
        get: |m| &mut m.layout_presets,
    },
    ModuleInfo {
        name: "Custom themes",
        category: ModuleCategory::Appearance,
        description: "Import and apply your own themes (Settings, Themes). Off = only the built-in Dark and Light themes, and no theme files are read.",
        get: |m| &mut m.custom_themes,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// polish-2's default config, before any Release A module existed.
    const POLISH2_CONFIG: &str = r#"{
        "theme": "DiscordDark", "zoom": 1.0,
        "modules": { "layout_editor": true, "images": true, "emoji_images": true,
            "typing_indicators": true, "member_list": true, "notifications": true,
            "status_bar": false, "embeds": true, "autocomplete": true, "gif_picker": false },
        "remember_login": true, "saved_token": null, "max_messages_per_channel": 200,
        "cached_channels": 8, "warn_before_links": true, "trusted_domains": [],
        "disable_formatting": false, "use_12_hour_time": false, "show_embed_images": true,
        "mention_on_reply": true, "show_blocked_messages": false, "compact_messages": false,
        "client_identity": "Web", "desktop_notifications": true, "flash_taskbar": true,
        "compact_member_list": false, "bot_member_intents": false
    }"#;

    #[test]
    fn old_config_still_loads() {
        let c: Config = serde_json::from_str(POLISH2_CONFIG).expect("polish-2 config loads");
        // Old values are kept, new modules default on.
        assert!(!c.modules.gif_picker);
        let d = Modules::default();
        for m in MODULES {
            let mut a = c.modules.clone();
            let mut b = d.clone();
            if m.name != "GIF picker" {
                assert_eq!(*(m.get)(&mut a), *(m.get)(&mut b), "{}", m.name);
            }
        }
        assert!(c.modules.reactions);
        assert_eq!(c.custom_theme, None);
        assert!(c.modules.custom_themes);
    }

    #[test]
    fn every_module_has_a_listed_category() {
        for m in MODULES {
            assert!(ModuleCategory::ALL.contains(&m.category), "{}", m.name);
        }
        for c in ModuleCategory::ALL {
            assert!(MODULES.iter().any(|m| m.category == c), "{c:?} is empty");
        }
    }

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
        assert_eq!(desk["release_channel"], "stable");
        assert_eq!(desk["os_arch"], "x64");
        assert!(desk["client_version"].as_str().is_some());
        assert_eq!(
            ClientIdentity::Desktop.properties("macos")["os"],
            "Mac OS X"
        );
        assert_eq!(ClientIdentity::Desktop.properties("linux")["os"], "Linux");
        let mobile = ClientIdentity::Mobile.properties("linux");
        assert_eq!(mobile["browser"], "Discord Android");
        assert_eq!(mobile["os"], "Android");
        assert_eq!(mobile["release_channel"], "googleRelease");
        assert!(!mobile["device"].as_str().unwrap().is_empty());
        for p in [&desk, &mobile] {
            for k in [
                "release_channel",
                "client_version",
                "os_version",
                "os_arch",
                "system_locale",
            ] {
                assert!(p[k].as_str().is_some_and(|v| !v.is_empty()), "{k} in {p}");
            }
        }
        // Old configs without the field stay on Web.
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.client_identity, ClientIdentity::Web);
    }
}
