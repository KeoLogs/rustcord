# Handoff — where RustCord is (written 2026-11-09)

This file is for whoever (human or Claude Code session) continues the work.
Read CLAUDE.md first, then this.

## Merged-into-each-other PR stack (merge in order on GitHub)
#1 feat/chat-parity → #2 feat/settings-profile-uploads → #3 feat/notifications →
#4 feat/members-presence → #5 feat/polish → #6 feat/messenger-complete
(each PR's base is the previous branch; merging in order retargets the next one to main)

## In progress when this was written
- feat/polish-2 (UI pass 2: Phosphor vector icons, settings draft + Save bar,
  status menu rows, Show-as fresh IDENTIFY + fuller properties, sysmsg/stickers,
  GIF picker, Unicode/symbol fonts, Discord + Discord Messenger comparison).
  Base: feat/messenger-complete. Check whether its PR exists before redoing it.
- wip/voice (voice groundwork: docs/VOICE.md, src/voice/ protocol/state/widgets).
- wip/release DONE @173c144 (base feat/messenger-complete): .github/workflows/release.yml
  (Run workflow with a version → builds Win zip + Inno setup.exe, Linux tar.gz, macOS
  universal .app zip, SHA256SUMS, GitHub Release), build.yml actions bumped to node24
  versions, build.rs + winresource (icon/version in exe), windows_subsystem, window
  icon, assets/icon/, packaging/, docs/RELEASING.md. Untested on Win/macOS (CI only).
  Suggest first run with a pre-release version e.g. 0.1.1-test.1.

## Finished modules waiting to be wired in (branches on origin)
Built as self-contained new files; none touch app.rs/layout.rs/gateway.rs/types.rs.
Wiring notes for each (ToUi/ToBackend variants, gateway events, hook points)
are below. Base noted per branch (feat/polish = older, merge carefully).

## settings-proto (wip/settings-proto @ 694d6a5)
- Decision: fetch via api.settings_proto() (GET users/@me/settings-proto/1) after READY for user accounts; no capabilities change in gateway.
- Needs: ToUi::UserSettings(Box<UserSettings>), ToBackend::PatchSettingsProto{settings_b64}, gateway dispatch USER_SETTINGS_PROTO_UPDATE -> apply_update_event.
- Status change: PATCH + gateway op 3 presence (verify).
- New deps prost 0.14, base64 0.22 (Cargo conflicts possible when merging).
- Patches only from explicit user actions.
## embeds-pins (wip/embeds-pins @ 04d4e05)
- types.rs: Message.embeds (parse_embeds), MessagePatch.embeds (embeds_patch) + apply.
- ToBackend: LoadPins, Pin, Unpin. ToUi: Pins, PinsFailed, Unpinned, UnpinFailed. Gateway: CHANNEL_PINS_UPDATE -> reload if popup open.
- config: trusted_domains, show_embeds, show_embed_images.
- app: route attachment open_url + hyperlink_to through links::check / LinkWarning.
- Evict: embed.image_uris() -> forget_image.
- Gaps: can_unpin perms (DM or MANAGE_MESSAGES), "Pin message" menu item, embed/pins timestamps UTC -> use timefmt, clippy --all-targets items_after_test_module in gateway.rs.
## markdown (wip/markdown @ 5b8e302)
- md_cache: ParseCache in State; get(&m.id,&m.content,formatting_enabled); remove on delete; clear on evict.
- render_with(ui,&blocks,&RenderCtx,trailing) -> RenderOutput{action,response}; context menu on response; actions Link->links::check, User{id,rect}->profile popout, Channel->select.
- Lookup trait over roles/members nicks/channels/mentions. Pass bold_family if polish registered a bold font.
- emoji::unicode_segments added. chrono dep added (polish may also add chrono/time -> Cargo conflict).
- Use to_plain_text for reply previews + notifications.
- Settings: DisableFormatting toggle (config).
- Gap: masked link warning must show real URL.
## threads (wip/threads @ bee5fc8, base feat/polish)
- ToUi: Thread(ThreadEvent), ThreadPage{page}, ForumPage{forum_id,query,page}->apply_forum_page, ForumPageFailed->fail_load, ThreadCreated(Thread)->Upsert+open, ThreadJoined{thread_id,joined}->set_joined
- ToBackend: LoadActiveThreads{guild_id}, LoadForumThreads{guild_id,forum_id,query} (call store.begin_load when sending), LoadArchivedThreads{channel_id,before}, JoinThread, LeaveThread, CreateThread{..}, CreatePost{..}
- Gateway: THREAD_* via ThreadEvent::from_dispatch; from_guild on READY guilds + GUILD_CREATE; MESSAGE_CREATE->store.on_message; GUILD_DELETE->remove_guild; CHANNEL_DELETE->remove_parent; set_me after READY.
- types: Channel::is_listed hides 10-16 -> show forums (open ForumView); threads openable as channels (resolve via store.get).
- UI: thread_rows after parent channel row; ForumView for forum channels; ThreadDialog from message menu ("Create thread"); thread_browser popout.
- Uncertain: /threads/search for user forums; /guilds/{id}/threads/active bot-only.
- Not visually verified at all.
## reactions + emoji picker + symbols (wip/reactions @ f76e3f3, base feat/polish)
- types: Message.reactions = reactions::parse(&v["reactions"]) (+ MESSAGE_UPDATE?); ToBackend::React{channel_id,message_id,emoji:EmojiRef,add,burst}, LoadReactors{..}; ToUi::Reaction(ReactionEvent), ReactionFailed{..}, Reactors{..}
- gateway: MESSAGE_REACTION_ADD/REMOVE/REMOVE_ALL/REMOVE_EMOJI via ReactionEvent::from_dispatch; bot intents add 1<<10 and 1<<13.
- app: reaction_bar under message body; pending.begin then send; events via pending.on_event; failures on_failure; prune periodically; WantReactors dedupe; persist RecentEmoji in Config; CustomEmojiGroup from READY guild emojis + GUILD_EMOJIS_UPDATE; "Reactions" module.
- Hover bar: add-reaction button -> reactions::picker_popup. Composer emoji button (polish-2 stub ComposerAction::OpenEmojiPicker) -> emoji_picker::popup with allow_text:true (Symbols tab).
- emoji.rs: twemoji_url, custom_emoji_url added (possible conflict with polish-2/markdown changes).
- Gaps: no super reaction add, no 429 handling, no skin tones.
## search (wip/search @ 5db6d56, base feat/polish)
- ToBackend::Search{seq,scope,query,offset}; ToUi::SearchResults{seq,total,messages,partial}, SearchIndexing{seq,retry_after,documents_indexed}, SearchFailed{seq,error}. Jump via State::jump_to.
- Ctrl+F -> panel.open(Some((channel_id,name))) / server-wide None; popup list_height Some(420). Module flag + settings entry. Results plain text (could use markdown). Not visually verified.
## friends + home (wip/friends @ ed33bed, base feat/polish)
- ToUi: Relationships(Vec), MessageRequests(Vec<RequestInfo>) from READY; RelationshipEvent for RELATIONSHIP_ADD/UPDATE/REMOVE; MessageRequestUpdate on CHANNEL_CREATE/UPDATE (+remove on CHANNEL_DELETE); FriendResult(Result<Outcome,String>); RequestResult{action,result}
- ToBackend: Friends(FriendsAction), MessageRequest(RequestAction) -> friends::execute / execute_request.
- UI: home_header atop DM list (replace "Direct Messages" header); Friends/Requests pages replace Messages pane; filter DM list with requests.visible_dms + home::filter_dms; open existing DM before API; don't ack request channels. AvatarFn = app::paint_avatar.
## themes (wip/themes @ dec6e1b, base messenger-complete)
- See agent report steps: app startup ThemeStore+ActiveTheme::from_config; apply_theme uses active.apply, rebuild on (theme,custom_theme,modules.custom_themes); pal = active.palette; layout pane_ui: active.paint_pane_background + fill(active.pane_fill); settings Section::Themes -> ThemesPage::show (+refresh on open), actions ApplyBuiltin/Apply/Delete/OpenFolder; ToBackend::PickTheme{folder} -> ToUi::ThemePicked(Option<PathBuf>) -> store.import.
- Watch: direct pal.chat/sidebar fills in app.rs override gradients; Config gets custom_theme + Modules.custom_themes (config.rs conflict with polish-2 draft settings).
## onboarding/invites/screening (wip/onboarding @ 4450fc5, base messenger-complete)
- Modules::invite_cards; State: InviteCache + dialogs. invite_card under message content per extract_invite_codes; rail_add_button after guilds -> AddServerDialog. After Joined: onboarding then screening. pending_banner above composer (disable send). Guild ctx menu "Channels & Roles" -> ChannelsRoles page.
- ToBackend: ResolveInvite, JoinInvite, LoadScreening, SubmitScreening{guild_id,body}, LoadOnboarding, SubmitOnboarding{guild_id,body}
- ToUi: InviteResolved, InviteJoined, ScreeningLoaded, ScreeningSubmitted, OnboardingLoaded, OnboardingSaved
- types/gateway: Guild.features, my_pending, my_member_flags (READY merged_members, GUILD_MEMBER_UPDATE for me).
## layout presets (wip/layout-presets @ 20ec945)
- Settings→Layout: preset_picker(ui,&pal,detect(&tree)) -> SettingsAction::ApplyPreset(p) -> tree=build(p). Copy layout (export) / Import layout (import -> paint_preview -> confirm). default_tree() could = build(Preset::Discord). Use sanitize() when loading saved layouts. Tab titles in layout.rs still emoji -> icon font.
## frosted glass test theme committed to wip/themes
## plugins (wip/plugins @ 2bd601f, base messenger-complete)
- Config: Modules::plugins (default false) + MODULES entry (text in src/plugins/mod.rs docs).
- App: plugins: PluginHost::new(); logic: plugins.update(ctx, modules.plugins) + drain_notifications -> toasts; Ready->on_ready, NewMessage->on_message, MessageUpdated->on_message_update, channel open->on_channel_open.
- messages_pane first pass: transform_display(&msg).unwrap_or(content); clear md_cache when display_generation() changes.
- submit: parse_command(draft) & run_command -> Draft into composer (never send). commands() feed "/" autocomplete.
- Pane::Plugins tile -> draw_panels(ui,&pal). Settings→Plugins: settings_ui(ui, draft.modules.plugins, &pal) -> Notice toast / SetModule. App::save -> plugins.save().
## user-settings (wip/user-settings @ 3fefd46, base messenger-complete)
- ToBackend::UserSettings(Request) -> backend::execute -> ToUi::UserSettings(Outcome) -> pages.apply (+ dialog.apply). NotificationsSaved Ok -> GuildSettings::from_json -> Mutes::update.
- State holds Pages + NotificationDialog (open from server dropdown / channel ctx menu). Config::app_options: AppOptions. Pages into settings.rs (Profiles, Devices, Connections, Keybinds, Windows/Advanced options, Language). forget_images when settings close. Ctx.current_session_hash = READY auth_session_id_hash.
- Wire AppOptions: window persistence (main.rs), api_base/cdn_base usage, verify_tls in reqwest+tungstenite, CheckUpdates on startup. Tray/autostart TODO.
## server-admin (wip/server-admin @ 559e580, base messenger-complete)
- Header click -> Popup::menu with menu::server_menu; ServerAdmin in State; on_menu returns NotificationSettings(->user_settings NotificationDialog)/Privacy/EditServerProfile/SetHideMutedChannels/LeaveServer to caller.
- Each frame: if covers_window() draw admin.show instead of tiles; else still call show (modals). ExternalPages trait for Roles/Members/Bans/Emoji/AuditLog/Onboarding (roles-mod + onboarding pages).
- ToBackend::Server(ServerRequest)->rest::execute->ToUi::Server(ServerResponse)->admin.handle; ToBackend::PickServerIcon->ToUi::ServerIconPicked->admin.set_icon. api: json_request (dedupe with user-settings request_json).
- Keep Guild channels/roles/overwrites live: CHANNEL_*, GUILD_UPDATE, GUILD_ROLE_*. Channel ctx menu "Edit Channel" -> open_channel_settings. close_all on GUILD_DELETE. Restyle with Palette::menu_bg after merge.
## roles-mod (wip/roles-mod @ 6ebd569, base messenger-complete)
- NOTE name clash: guild_admin::ServerAdmin vs server-admin's server::ServerAdmin -> rename (e.g. GuildAdminPages) and plug into server settings ExternalPages for Roles/Members/Bans/Emoji/AuditLog.
- ToBackend::GuildAdmin(AdminAction) -> guild_admin::execute -> ToUi::GuildAdmin{action,result} -> on_result.
- member/author right-click: moderation::member_context_menu -> admin.dialogs.handle -> draw dialogs.show every frame.
- roles.apply_gateway for GUILD_ROLE_*; AdminCtx.name_of from MemberCache. api: admin_request (third generic request helper -> consolidate json_request/request_json/admin_request).

Plus wip/themes also contains examples/themes/frosted-glass (converted from
DiscordStyles/FrostedGlass, MIT, original generated background).

## Plan ("waves", max 6 parallel agents — Leo's limit)
1. Finish in-progress branches above.
2. Release A integration (one agent, on top of feat/polish-2): reactions + emoji
   picker (+ Symbols tab), search (Ctrl+F), friends + home header + message
   requests, threads & forums, onboarding/invites/screening, layout presets,
   themes (+ Settings → Themes). Then Leo tests on real Discord.
3. Release B integration: server-admin (dropdown, server settings shell,
   channels, invites), roles-mod pages plugged into the shell, user-settings
   pages, plugins (Luau). Consolidate the three generic api request helpers.
4. Voice (DAVE + Opus), after groundwork.
5. GitHub Release via wip/release workflow.

## Leo's standing requests / preferences
- Vector icons only, never emoji as UI icons.
- Discord-like look; also borrow Discord Messenger ideas; keep the layout editor.
- Low RAM; optional features are Modules (off = nothing loaded).
- Themes: .rctheme; test themes live in examples/themes only (not in releases).
- "Show as" Web/Desktop/Mobile is opt-in with a warning (Web default).
- Full file contents (not diffs) when showing code in chat.
- PRs, never push to main. No self-bot behaviour.
