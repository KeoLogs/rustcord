//! Backend side of the user settings pages: runs a [`Request`] and returns
//! the [`Outcome`] for the UI. Call from the backend's request loop, e.g.
//!
//! ```ignore
//! ToBackend::UserSettings(req) => {
//!     let out = user_settings::backend::execute(&api, req).await;
//!     let _ = tx.send(ToUi::UserSettings(out));
//!     ctx.request_repaint();
//! }
//! ```
//!
//! Every request here comes from a click or from opening a page; nothing
//! runs on its own.

use anyhow::Result;
use reqwest::Method;
use serde_json::{Value, json};

use super::connections::Connection;
use super::options::{self, UpdateInfo};
use super::profiles::{self, ImageSlot, MemberProfile, MyProfile, PickedImage};
use super::sessions::AuthSession;
use super::{Outcome, Request};
use crate::api::Api;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub async fn execute(api: &Api, req: Request) -> Outcome {
    match req {
        Request::LoadProfile => Outcome::Profile(load_profile(api).await.map_err(err)),
        Request::PickImage(slot) => Outcome::ImagePicked {
            slot,
            result: pick_image(slot).await,
        },
        Request::SaveProfile { user, profile } => {
            Outcome::ProfileSaved(save_profile(api, user, profile).await.map_err(err))
        }
        Request::LoadMember { guild_id, user_id } => {
            let result = api
                .user_profile(&user_id, Some(&guild_id))
                .await
                .map(|v| MemberProfile::from_json(&v))
                .map_err(err);
            Outcome::Member { guild_id, result }
        }
        Request::SaveMember { guild_id, body } => {
            let result = api
                .request_json(
                    Method::PATCH,
                    &format!("/guilds/{guild_id}/members/@me"),
                    Some(&body),
                    "Saving server profile",
                )
                .await
                .map(|v| MemberProfile::from_json(&v))
                .map_err(err);
            Outcome::MemberSaved { guild_id, result }
        }
        Request::SaveNotifications { guild_id, body } => {
            let result = api
                .request_json(
                    Method::PATCH,
                    &format!("/users/@me/guilds/{guild_id}/settings"),
                    Some(&body),
                    "Saving notification settings",
                )
                .await
                .map_err(err);
            Outcome::NotificationsSaved { guild_id, result }
        }
        Request::LoadSessions => Outcome::Sessions(
            api.request_json(Method::GET, "/auth/sessions", None, "Loading devices")
                .await
                .map(|v| AuthSession::parse_list(&v))
                .map_err(err),
        ),
        Request::LogoutSessions(hashes) => Outcome::SessionsLoggedOut(
            api.request_json(
                Method::POST,
                "/auth/sessions/logout",
                Some(&json!({ "session_id_hashes": hashes })),
                "Logging out device",
            )
            .await
            .map(|_| hashes)
            .map_err(err),
        ),
        Request::LoadConnections => Outcome::Connections(
            api.request_json(
                Method::GET,
                "/users/@me/connections",
                None,
                "Loading connections",
            )
            .await
            .map(|v| Connection::parse_list(&v))
            .map_err(err),
        ),
        Request::CheckUpdates => Outcome::UpdateCheck(check_updates().await),
    }
}

async fn load_profile(api: &Api) -> Result<MyProfile> {
    let user = api
        .request_json(Method::GET, "/users/@me", None, "Loading your account")
        .await?;
    let id = user.get("id").and_then(Value::as_str).unwrap_or_default();
    // The profile part (bio, pronouns) is optional: fall back to the user
    // object if it fails.
    let profile = api.user_profile(id, None).await.ok();
    Ok(MyProfile::from_json(&user, profile.as_ref()))
}

async fn save_profile(api: &Api, user: Option<Value>, profile: Option<Value>) -> Result<MyProfile> {
    if let Some(body) = &user {
        api.request_json(Method::PATCH, "/users/@me", Some(body), "Saving profile")
            .await?;
    }
    if let Some(body) = &profile {
        let r = api
            .request_json(
                Method::PATCH,
                "/users/@me/profile",
                Some(body),
                "Saving profile",
            )
            .await;
        if let Err(e) = r {
            // Older API shape: bio / accent_color on users/@me.
            let text = e.to_string();
            if text.contains("HTTP 404") || text.contains("HTTP 405") {
                api.request_json(Method::PATCH, "/users/@me", Some(body), "Saving profile")
                    .await?;
            } else {
                return Err(e);
            }
        }
    }
    load_profile(api).await
}

/// Native file picker, then the checks in [`profiles::check_image`].
pub async fn pick_image(slot: ImageSlot) -> Result<Option<PickedImage>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title(format!("Choose your {}", slot.label()))
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    // Check the size before reading a huge file into memory.
    let len = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("Couldn't open that file: {e}"))?
        .len();
    if len > profiles::MAX_IMAGE_BYTES as u64 {
        return Err(format!(
            "That file is {:.1} MB. Images must be under 10 MB.",
            len as f64 / 1_048_576.0
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Couldn't read that file: {e}"))?;
    let name = file.file_name();
    tokio::task::spawn_blocking(move || profiles::check_image(&name, bytes))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

/// GitHub's latest release of KeoLogs/rustcord vs. this build. Uses its own
/// client: no Discord token, ever.
pub async fn check_updates() -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("RustCord/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(options::RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("No RustCord releases have been published yet.".into());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub answered HTTP {}.", resp.status()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("Unexpected answer from GitHub: {e}"))?;
    options::parse_release(&v, env!("CARGO_PKG_VERSION"))
}
