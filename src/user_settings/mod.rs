//! The rest of Discord's user settings, as standalone pages the integrator
//! adds to Settings (`src/settings.rs` is not touched here).
//!
//! Every page is data-in / actions-out: `show(ui, &Ctx) -> Option<Action>`.
//! Pages never do I/O. Network work is an [`Action::Request`], which the
//! integrator sends to the backend (suggested: `ToBackend::UserSettings(req)`),
//! runs with [`backend::execute`] and sends back as an [`Outcome`] (suggested:
//! `ToUi::UserSettings(outcome)`), which goes into [`Pages::apply`] (and
//! [`NotificationDialog::apply`] while that dialog is open).
//!
//! Pages:
//! - [`profiles`]: display name, pronouns, about me, avatar / banner, banner
//!   colour, live preview card, and per-server profile (nickname, server
//!   avatar).
//! - [`notifications`]: per-server notification dialog (mute with duration,
//!   level, suppress @everyone / roles, mobile push, channel overrides) and
//!   the per-channel variant.
//! - [`sessions`]: Devices (logged-in sessions, log out with confirm).
//! - [`connections`]: read-only list of connected accounts.
//! - [`keybinds`]: static table of shortcuts (extendable).
//! - [`options`]: Discord Messenger's Window / Connection options we lacked
//!   (`AppOptions`) plus the update checker.
//! - [`language`]: language picker placeholder (English only).
//!
//! Nothing here acts on its own: every write (PATCH / POST) is the result of
//! a click, and destructive ones (log out a device, change the API URL, turn
//! off TLS checks) sit behind a confirm dialog. Loading pages (GET) happens
//! when a page is first shown.
//!
//! Protocol notes and uncertainties are in each page's module docs.

pub mod backend;
pub mod connections;
pub mod keybinds;
pub mod language;
pub mod notifications;
pub mod options;
pub mod profiles;
pub mod sessions;
mod widgets;

use crate::theme::Palette;
use crate::types::{Guild, User};

pub use connections::ConnectionsPage;
pub use keybinds::KeybindsPage;
pub use language::LanguagePage;
pub use notifications::NotificationDialog;
pub use options::{AppOptions, OptionsPage};
pub use profiles::ProfilesPage;
pub use sessions::SessionsPage;

/// What the pages need from the app each frame.
pub struct Ctx<'a> {
    pub pal: &'a Palette,
    /// The logged-in user (None while connecting).
    pub me: Option<&'a User>,
    /// Bot accounts have no profile / notification / sessions / connections
    /// endpoints; the pages say so instead of loading.
    pub is_bot: bool,
    /// Servers, for the per-server profile picker and the notification
    /// dialog's channel names.
    pub guilds: &'a [Guild],
    /// READY `auth_session_id_hash` (user accounts), to mark "This device"
    /// in the Devices list. None if unknown.
    pub current_session_hash: Option<&'a str>,
}

/// What a page asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Run in the backend with [`backend::execute`]; feed the result to
    /// [`Pages::apply`].
    Request(Request),
    /// Open a link through `State::open_link` (link warning applies).
    OpenLink(String),
    /// The custom API / CDN URLs were changed (and confirmed) on the Options
    /// page. The integrator must clear the saved token and log out, so the
    /// Discord token is never sent to the new host.
    LogOutForNewBackend,
    /// The notification dialog was closed (saved or cancelled).
    CloseDialog,
}

/// Backend work. Writes are only ever created by a click.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `GET users/@me` + `GET users/{me}/profile`.
    LoadProfile,
    /// Native file picker for an avatar / banner (runs off the UI thread),
    /// then size / type checks.
    PickImage(profiles::ImageSlot),
    /// `PATCH users/@me` and/or `PATCH users/@me/profile` (either may be None).
    SaveProfile {
        user: Option<serde_json::Value>,
        profile: Option<serde_json::Value>,
    },
    /// My member data in a server (nick, server avatar) via
    /// `GET users/{me}/profile?guild_id=`.
    LoadMember { guild_id: String, user_id: String },
    /// `PATCH guilds/{id}/members/@me`.
    SaveMember {
        guild_id: String,
        body: serde_json::Value,
    },
    /// `PATCH users/@me/guilds/{id}/settings` (`@me` for DMs).
    SaveNotifications {
        guild_id: String,
        body: serde_json::Value,
    },
    /// `GET auth/sessions`.
    LoadSessions,
    /// `POST auth/sessions/logout`.
    LogoutSessions(Vec<String>),
    /// `GET users/@me/connections`.
    LoadConnections,
    /// GitHub releases of KeoLogs/rustcord (unauthenticated, no token).
    CheckUpdates,
}

/// Result of a [`Request`], back on the UI thread. Errors are user-facing
/// strings.
#[derive(Debug, Clone)]
pub enum Outcome {
    Profile(Result<profiles::MyProfile, String>),
    ImagePicked {
        slot: profiles::ImageSlot,
        /// Ok(None) = the picker was cancelled.
        result: Result<Option<profiles::PickedImage>, String>,
    },
    /// The new user object / profile metadata (whichever were sent).
    ProfileSaved(Result<profiles::MyProfile, String>),
    Member {
        guild_id: String,
        result: Result<profiles::MemberProfile, String>,
    },
    MemberSaved {
        guild_id: String,
        result: Result<profiles::MemberProfile, String>,
    },
    /// The updated user guild settings entry: feed `Ok` values to
    /// `notify::GuildSettings::from_json` → `Mutes::update` (a
    /// `USER_GUILD_SETTINGS_UPDATE` follows on the gateway too).
    NotificationsSaved {
        guild_id: String,
        result: Result<serde_json::Value, String>,
    },
    Sessions(Result<Vec<sessions::AuthSession>, String>),
    SessionsLoggedOut(Result<Vec<String>, String>),
    Connections(Result<Vec<connections::Connection>, String>),
    UpdateCheck(Result<options::UpdateInfo, String>),
}

/// A value loaded from Discord on demand.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Loadable<T> {
    #[default]
    NotLoaded,
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> Loadable<T> {
    /// First call returns true (and marks it Loading): send the load request.
    pub fn start(&mut self) -> bool {
        if matches!(self, Loadable::NotLoaded) {
            *self = Loadable::Loading;
            true
        } else {
            false
        }
    }

    pub fn set(&mut self, r: Result<T, String>) {
        *self = match r {
            Ok(v) => Loadable::Loaded(v),
            Err(e) => Loadable::Failed(e),
        };
    }

    pub fn get(&self) -> Option<&T> {
        match self {
            Loadable::Loaded(v) => Some(v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        match self {
            Loadable::Loaded(v) => Some(v),
            _ => None,
        }
    }
}

/// All pages together, so the integrator keeps one value in `State` and
/// routes every [`Outcome`] through [`Pages::apply`].
#[derive(Default)]
pub struct Pages {
    pub profiles: ProfilesPage,
    pub sessions: SessionsPage,
    pub connections: ConnectionsPage,
    pub keybinds: KeybindsPage,
    pub options: OptionsPage,
    pub language: LanguagePage,
}

impl Pages {
    /// Store a backend result in the page it belongs to. Gives back the
    /// outcomes meant for the notification dialog
    /// ([`NotificationDialog::apply`]; also update `Mutes` from them).
    pub fn apply(&mut self, ctx: &eframe::egui::Context, outcome: Outcome) -> Option<Outcome> {
        match outcome {
            Outcome::Profile(_)
            | Outcome::ImagePicked { .. }
            | Outcome::ProfileSaved(_)
            | Outcome::Member { .. }
            | Outcome::MemberSaved { .. } => self.profiles.apply(ctx, outcome),
            Outcome::Sessions(_) | Outcome::SessionsLoggedOut(_) => self.sessions.apply(outcome),
            Outcome::Connections(r) => self.connections.list.set(r),
            Outcome::UpdateCheck(r) => self.options.apply_update(r),
            Outcome::NotificationsSaved { .. } => return Some(outcome),
        }
        None
    }

    /// Free images held by drafts (call when Settings closes).
    pub fn forget_images(&mut self, ctx: &eframe::egui::Context) {
        self.profiles.forget_images(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadable_starts_once() {
        let mut l: Loadable<u8> = Loadable::default();
        assert!(l.start());
        assert!(!l.start());
        l.set(Ok(3));
        assert_eq!(l.get(), Some(&3));
        assert!(!l.start());
        l.set(Err("x".into()));
        assert_eq!(l, Loadable::Failed("x".into()));
    }
}
