//! Backend side of the server admin module: the REST requests the windows
//! ask for ([`ServerRequest`]), their answers ([`ServerResponse`]) and
//! [`execute`], which runs one request on the backend's Tokio runtime.
//!
//! Every request here comes from an explicit user click (Save, Create,
//! Revoke after a confirm, …) or is a read (GET). Nothing is sent on its own.
//!
//! Endpoints (Discord API v10):
//! - `GET/PATCH /guilds/{id}`, `GET /guilds/{id}/invites`
//! - `POST /guilds/{id}/channels`, `PATCH /guilds/{id}/channels` (positions)
//! - `GET/PATCH/DELETE /channels/{id}`, `GET/POST /channels/{id}/invites`
//! - `PUT/DELETE /channels/{id}/permissions/{overwrite_id}`
//! - `DELETE /invites/{code}`

use base64::Engine;
use reqwest::Method;
use serde_json::Value;

use super::invites::Invite;
use super::overwrites::OverwriteChange;
use crate::api::Api;
use crate::perms::Overwrite;

/// Largest image accepted for a server icon (Discord's upload limit for
/// icons is 10 MiB).
pub const MAX_ICON_BYTES: usize = 10 * 1024 * 1024;

/// A REST request from one of the server admin windows.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerRequest {
    /// `GET /guilds/{id}`: full settings for the Overview page.
    LoadGuild { guild_id: String },
    /// `PATCH /guilds/{id}` with only the changed fields.
    PatchGuild { guild_id: String, body: Value },
    /// `GET /guilds/{id}/invites` (Manage Server).
    LoadGuildInvites { guild_id: String },
    /// `GET /channels/{id}/invites` (Manage Channels).
    LoadChannelInvites { channel_id: String },
    /// `POST /channels/{id}/invites`.
    CreateInvite { channel_id: String, body: Value },
    /// `DELETE /invites/{code}` (after a confirm).
    RevokeInvite { code: String },
    /// `GET /channels/{id}`: nsfw / slowmode / bitrate etc. for the editor.
    LoadChannel { channel_id: String },
    /// `POST /guilds/{id}/channels`.
    CreateChannel { guild_id: String, body: Value },
    /// `PATCH /channels/{id}` with only the changed fields.
    PatchChannel { channel_id: String, body: Value },
    /// `DELETE /channels/{id}` (after a confirm).
    DeleteChannel { channel_id: String },
    /// `PATCH /guilds/{id}/channels` with `[{id, position, parent_id?}]`.
    ReorderChannels { guild_id: String, body: Value },
    /// PUT / DELETE `channels/{id}/permissions/{overwrite_id}`, one call per
    /// change, in order. `result` is the full list after saving (sent back
    /// in [`ServerResponse::OverwritesSaved`]).
    SaveOverwrites {
        channel_id: String,
        changes: Vec<OverwriteChange>,
        result: Vec<Overwrite>,
    },
}

impl ServerRequest {
    /// Short description for errors ("Saving server settings failed: …").
    pub fn what(&self) -> &'static str {
        match self {
            ServerRequest::LoadGuild { .. } => "Loading server settings",
            ServerRequest::PatchGuild { .. } => "Saving server settings",
            ServerRequest::LoadGuildInvites { .. } | ServerRequest::LoadChannelInvites { .. } => {
                "Loading invites"
            }
            ServerRequest::CreateInvite { .. } => "Creating invite",
            ServerRequest::RevokeInvite { .. } => "Revoking invite",
            ServerRequest::LoadChannel { .. } => "Loading channel",
            ServerRequest::CreateChannel { .. } => "Creating channel",
            ServerRequest::PatchChannel { .. } => "Saving channel",
            ServerRequest::DeleteChannel { .. } => "Deleting channel",
            ServerRequest::ReorderChannels { .. } => "Moving channels",
            ServerRequest::SaveOverwrites { .. } => "Saving permissions",
        }
    }
}

/// The answer to a [`ServerRequest`]; feed it to `ServerAdmin::handle`.
#[derive(Debug, Clone)]
pub enum ServerResponse {
    /// Full guild object (from GET or PATCH).
    Guild {
        guild_id: String,
        data: Value,
    },
    GuildInvites {
        guild_id: String,
        invites: Vec<Invite>,
    },
    ChannelInvites {
        channel_id: String,
        invites: Vec<Invite>,
    },
    InviteCreated {
        channel_id: String,
        invite: Invite,
    },
    InviteRevoked {
        code: String,
    },
    /// Full channel object (from GET or PATCH).
    Channel {
        channel_id: String,
        data: Value,
    },
    ChannelCreated {
        guild_id: String,
        data: Value,
    },
    ChannelDeleted {
        channel_id: String,
    },
    Reordered {
        guild_id: String,
    },
    OverwritesSaved {
        channel_id: String,
        overwrites: Vec<Overwrite>,
    },
    Failed {
        request: Box<ServerRequest>,
        error: String,
    },
}

/// Discord ids are decimal snowflakes; anything else never reaches a URL.
pub fn is_snowflake(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Invite codes (and vanity codes) are short and URL-safe.
pub fn is_invite_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The ids a request puts into its URL, checked before anything is sent.
fn ids_ok(req: &ServerRequest) -> bool {
    match req {
        ServerRequest::LoadGuild { guild_id }
        | ServerRequest::PatchGuild { guild_id, .. }
        | ServerRequest::LoadGuildInvites { guild_id }
        | ServerRequest::CreateChannel { guild_id, .. }
        | ServerRequest::ReorderChannels { guild_id, .. } => is_snowflake(guild_id),
        ServerRequest::LoadChannelInvites { channel_id }
        | ServerRequest::CreateInvite { channel_id, .. }
        | ServerRequest::LoadChannel { channel_id }
        | ServerRequest::PatchChannel { channel_id, .. }
        | ServerRequest::DeleteChannel { channel_id } => is_snowflake(channel_id),
        ServerRequest::SaveOverwrites {
            channel_id,
            changes,
            ..
        } => is_snowflake(channel_id) && changes.iter().all(|c| is_snowflake(c.id())),
        ServerRequest::RevokeInvite { code } => is_invite_code(code),
    }
}

/// Run one request. Never panics; failures come back as
/// [`ServerResponse::Failed`].
pub async fn execute(api: &Api, req: ServerRequest) -> ServerResponse {
    if !ids_ok(&req) {
        return ServerResponse::Failed {
            error: "invalid id".into(),
            request: Box::new(req),
        };
    }
    match run(api, &req).await {
        Ok(r) => r,
        Err(e) => ServerResponse::Failed {
            error: format!("{e:#}"),
            request: Box::new(req),
        },
    }
}

async fn run(api: &Api, req: &ServerRequest) -> anyhow::Result<ServerResponse> {
    let what = req.what();
    Ok(match req {
        ServerRequest::LoadGuild { guild_id } => {
            let data = api
                .json_request(Method::GET, &format!("/guilds/{guild_id}"), None, what)
                .await?;
            ServerResponse::Guild {
                guild_id: guild_id.clone(),
                data,
            }
        }
        ServerRequest::PatchGuild { guild_id, body } => {
            let data = api
                .json_request(
                    Method::PATCH,
                    &format!("/guilds/{guild_id}"),
                    Some(body),
                    what,
                )
                .await?;
            ServerResponse::Guild {
                guild_id: guild_id.clone(),
                data,
            }
        }
        ServerRequest::LoadGuildInvites { guild_id } => {
            let v = api
                .json_request(
                    Method::GET,
                    &format!("/guilds/{guild_id}/invites"),
                    None,
                    what,
                )
                .await?;
            ServerResponse::GuildInvites {
                guild_id: guild_id.clone(),
                invites: Invite::list_from_json(&v),
            }
        }
        ServerRequest::LoadChannelInvites { channel_id } => {
            let v = api
                .json_request(
                    Method::GET,
                    &format!("/channels/{channel_id}/invites"),
                    None,
                    what,
                )
                .await?;
            ServerResponse::ChannelInvites {
                channel_id: channel_id.clone(),
                invites: Invite::list_from_json(&v),
            }
        }
        ServerRequest::CreateInvite { channel_id, body } => {
            let v = api
                .json_request(
                    Method::POST,
                    &format!("/channels/{channel_id}/invites"),
                    Some(body),
                    what,
                )
                .await?;
            let invite = Invite::from_json(&v)
                .ok_or_else(|| anyhow::anyhow!("Discord sent no invite code"))?;
            ServerResponse::InviteCreated {
                channel_id: channel_id.clone(),
                invite,
            }
        }
        ServerRequest::RevokeInvite { code } => {
            api.json_request(Method::DELETE, &format!("/invites/{code}"), None, what)
                .await?;
            ServerResponse::InviteRevoked { code: code.clone() }
        }
        ServerRequest::LoadChannel { channel_id } => {
            let data = api
                .json_request(Method::GET, &format!("/channels/{channel_id}"), None, what)
                .await?;
            ServerResponse::Channel {
                channel_id: channel_id.clone(),
                data,
            }
        }
        ServerRequest::CreateChannel { guild_id, body } => {
            let data = api
                .json_request(
                    Method::POST,
                    &format!("/guilds/{guild_id}/channels"),
                    Some(body),
                    what,
                )
                .await?;
            ServerResponse::ChannelCreated {
                guild_id: guild_id.clone(),
                data,
            }
        }
        ServerRequest::PatchChannel { channel_id, body } => {
            let data = api
                .json_request(
                    Method::PATCH,
                    &format!("/channels/{channel_id}"),
                    Some(body),
                    what,
                )
                .await?;
            ServerResponse::Channel {
                channel_id: channel_id.clone(),
                data,
            }
        }
        ServerRequest::DeleteChannel { channel_id } => {
            api.json_request(
                Method::DELETE,
                &format!("/channels/{channel_id}"),
                None,
                what,
            )
            .await?;
            ServerResponse::ChannelDeleted {
                channel_id: channel_id.clone(),
            }
        }
        ServerRequest::ReorderChannels { guild_id, body } => {
            api.json_request(
                Method::PATCH,
                &format!("/guilds/{guild_id}/channels"),
                Some(body),
                what,
            )
            .await?;
            ServerResponse::Reordered {
                guild_id: guild_id.clone(),
            }
        }
        ServerRequest::SaveOverwrites {
            channel_id,
            changes,
            result,
        } => {
            for change in changes {
                let path = format!("/channels/{channel_id}/permissions/{}", change.id());
                match change {
                    OverwriteChange::Put { .. } => {
                        api.json_request(Method::PUT, &path, Some(&change.put_body()), what)
                            .await?;
                    }
                    OverwriteChange::Delete { .. } => {
                        api.json_request(Method::DELETE, &path, None, what).await?;
                    }
                }
            }
            ServerResponse::OverwritesSaved {
                channel_id: channel_id.clone(),
                overwrites: result.clone(),
            }
        }
    })
}

/// Image bytes as the `data:` URI Discord takes for icons. Detects the type
/// from the file's magic bytes (falling back to the extension).
pub fn image_data_uri(bytes: &[u8], file_name: &str) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("The file is empty.".into());
    }
    if bytes.len() > MAX_ICON_BYTES {
        return Err("Images must be smaller than 10 MB.".into());
    }
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        match file_name
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return Err("Pick a PNG, JPEG, GIF or WebP image.".into()),
        }
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// The base64 payload of a `data:` URI, decoded (for the icon preview).
pub fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let (_, b64) = uri.strip_prefix("data:")?.split_once(";base64,")?;
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// Backend helper for `AdminOutput::PickIcon`: open the system file picker
/// (images only), read the file and turn it into a data URI. `None` if the
/// user cancelled.
pub async fn pick_icon_data_uri() -> Option<Result<String, String>> {
    let file = rfd::AsyncFileDialog::new()
        .set_title("Choose a server icon")
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
        .pick_file()
        .await?;
    let path = file.path().to_path_buf();
    let name = file.file_name();
    Some(match tokio::fs::metadata(&path).await {
        Ok(m) if m.len() as usize > MAX_ICON_BYTES => {
            Err("Images must be smaller than 10 MB.".into())
        }
        Ok(_) => match tokio::fs::read(&path).await {
            Ok(bytes) => image_data_uri(&bytes, &name),
            Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
        },
        Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_checks() {
        assert!(is_snowflake("123456789012345678"));
        assert!(!is_snowflake("12/../34"));
        assert!(!is_snowflake(""));
        assert!(is_invite_code("aBc-12_x"));
        assert!(!is_invite_code("abc/def"));
        assert!(!is_invite_code("a?b"));
        let bad = ServerRequest::RevokeInvite {
            code: "../users/@me".into(),
        };
        assert!(!ids_ok(&bad));
        let ok = ServerRequest::PatchChannel {
            channel_id: "42".into(),
            body: json!({}),
        };
        assert!(ids_ok(&ok));
    }

    #[test]
    fn data_uris() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2];
        let uri = image_data_uri(&png, "whatever.bin").unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        assert_eq!(decode_data_uri(&uri).unwrap(), png);
        assert!(
            image_data_uri(b"plain", "x.jpg")
                .unwrap()
                .starts_with("data:image/jpeg")
        );
        assert!(image_data_uri(b"plain", "x.txt").is_err());
        assert!(image_data_uri(b"", "x.png").is_err());
        assert!(decode_data_uri("https://x").is_none());
    }
}
