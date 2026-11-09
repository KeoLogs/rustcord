# RustCord plugins

Plugins are small [Luau](https://luau.org) scripts (the Lua dialect Roblox
uses) that add `/commands`, panels and local tweaks to RustCord. Each plugin
runs in its own sandboxed VM with only the permissions you approve.

- The **Plugins** module (Settings → Modules) must be on. When it's off, no
  plugin VM exists and nothing is loaded.
- **No plugins are installed by default.** `examples/plugins/` in the repo
  has three to try.
- Plugins **can't** send messages, react, join servers, change settings,
  use the network, read or write files, or see your token. They can read
  what they're given, change how messages look *on your PC*, add UI, and
  *propose* text for the message box, which you send yourself.

## Installing

Settings → Plugins → **Install plugin (.zip)…** or **Install from folder…**.
RustCord checks `plugin.toml`, shows what the plugin can do, and installs it
only after you approve. Updating (installing the same id again) replaces the
old files but keeps the plugin's saved data. If an update asks for new
permissions it stays off until you approve them (switch it off and on).

Plugins live in RustCord's data folder:

| OS | Plugins | Plugin data |
|---|---|---|
| Windows | `%APPDATA%\RustCord\data\plugins` | `…\data\plugin-data` |
| Linux | `~/.local/share/rustcord/plugins` | `~/.local/share/rustcord/plugin-data` |
| macOS | `~/Library/Application Support/RustCord/plugins` | `…/plugin-data` |

Each installed plugin has: an on/off switch, its permissions, version and
author, **Reload**, **Log**, **Folder** and **Uninstall** (deletes the folder
and its data after asking). **Reload all** re-reads the plugins folder, so
you can also develop a plugin directly in there.

## Plugin format

A folder named after the plugin id:

```
my-plugin/
  plugin.toml
  main.luau
  lib/util.luau      (optional, loaded with require("lib/util"))
```

`plugin.toml`:

```toml
id = "my-plugin"          # 1-64 of a-z 0-9 - _ ; must match the folder name
name = "My Plugin"
version = "1.0.0"
author = "you"
description = "What it does."
min_rustcord = "0.1.0"     # optional
permissions = ["commands", "storage"]
entry = "main.luau"        # default
```

Unknown permissions are rejected. Zips may contain the files directly or one
top folder holding them. Installs are capped at 8 MB / 512 files, and zips
with `..`, absolute paths, drive letters or symlinks are refused.

## Permissions

| Permission | Allows |
|---|---|
| `read_messages` | `message` and `message_update` events for channels you can see |
| `ui` | `rustcord.ui.panel` and `rustcord.ui.settings` |
| `commands` | `rustcord.command` (produces text for the message box) |
| `message_display` | `rustcord.display` (changes how messages look locally) |
| `storage` | `rustcord.storage`, and `ui:textbox` / `ui:checkbox` (bound to storage) |
| `notifications` | `rustcord.notify` |

Calling an API without its permission raises an error such as
`rustcord.command needs the "commands" permission in plugin.toml`.
`ready`, `channel_open`, `rustcord.log` and `print` need no permission.

## API

Everything is on the read-only `rustcord` table.

### `rustcord.on(event, fn)`

| Event | Argument | Permission |
|---|---|---|
| `"ready"` | `{ id, name, username, bot }` (your account) | — |
| `"channel_open"` | `{ id, guild_id, name }` (`guild_id` is nil in DMs) | — |
| `"message"` | message table (below) | `read_messages` |
| `"message_update"` | `{ id, channel_id, content, edited }` (`content` may be nil) | `read_messages` |

Message table: `{ id, channel_id, guild_id, author = { id, name, username, bot }, content, timestamp, edited }`
(`timestamp` is unix seconds; `guild_id` is nil in DMs).

```lua
rustcord.on("message", function(msg)
    if msg.content:find("rustcord") then
        rustcord.notify(msg.author.name .. " mentioned RustCord")
    end
end)
```

### `rustcord.command(name, description, fn(args, ctx) -> string?)`

Registers `/name` (1-32 of a-z 0-9 - _). When you type `/name some text`
and press Enter, `fn("some text", { channel_id, guild_id })` runs and the
string it returns **replaces the message box text**. Nothing is sent: you
review it and press Enter again. Returning `nil` clears the box.

```lua
rustcord.command("shrug", "Appends a shrug", function(args)
    return (args ~= "" and args .. " " or "") .. "¯\\_(ツ)_/¯"
end)
```

### `rustcord.display(fn(msg) -> string?)`

Changes how a message is shown on this PC. Return the text to show
(markdown works as usual), or `nil` to leave it. Results are cached per
message and recomputed when the message is edited or the plugin's storage
changes. With several display plugins, each gets the previous one's output.

```lua
rustcord.display(function(msg)
    return (msg.content:gsub("teh", "the"))
end)
```

### UI: `rustcord.ui.panel(title, fn(ui))` and `rustcord.ui.settings(fn(ui))`

`fn` runs every frame the panel is visible and describes the widgets; Rust
draws them (Lua never touches the UI library directly). `settings` adds a
"Settings" section under the plugin in Settings → Plugins. Methods (call
with `ui:` or `ui.`):

| Method | Widget |
|---|---|
| `ui:heading(text)` / `ui:label(text)` / `ui:small(text)` | text (small = muted) |
| `ui:separator()` | line |
| `ui:button(text, fn)` | button; `fn()` runs when clicked |
| `ui:textbox(key, hint?, multiline?)` | text box bound to `storage[key]` (needs `storage`) |
| `ui:checkbox(key, text)` | checkbox bound to `storage[key]` (needs `storage`) |
| `ui:image(url, w?, h?)` | image from `https://cdn.discordapp.com/` or `https://media.discordapp.net/` only |

```lua
rustcord.ui.panel("Counter", function(ui)
    local n = rustcord.storage.get("n") or 0
    ui:heading("Clicked " .. n .. " times")
    ui:button("Click", function() rustcord.storage.set("n", n + 1) end)
end)
```

### `rustcord.storage.get(key)` / `rustcord.storage.set(key, value)`

A per-plugin key-value store saved as JSON. Values: strings, numbers,
booleans, tables of those; `set(key, nil)` deletes. Keys are 1-128 bytes and
the whole store is capped at **256 KB** (`set` errors when full).

### `rustcord.notify(text)`

Shows a small notice inside RustCord (max 300 characters, 10 queued).

### `rustcord.log(...)` / `print(...)`

Writes to the plugin's log (Settings → Plugins → Log, last 200 lines).
Errors from the plugin show there too.

### Other

- `rustcord.version`: RustCord's version. `rustcord.plugin`: `{ id, name, version }`.
- `require("name")` loads `name.luau` (or `.lua`) from the plugin's own
  folder (`a-z A-Z 0-9 - _` segments joined by `/`; nothing else).
- Standard libraries: `string`, `table`, `math`, `utf8`, `bit32`,
  `buffer`, `coroutine`, and `os.time` / `os.date` / `os.clock`.
  Not available: `io`, `debug`, `package`, `loadstring`, `load`,
  `getfenv` / `setfenv`, `os.execute` and friends.

## Limits and errors

- **Memory**: 16 MB per plugin.
- **Time**: each call into a plugin (loading, an event, a command, a display
  call, building a panel, a button) may run 50 ms; then it's stopped with an
  error. All plugins together get 100 ms per frame for panels and message
  display; past that they're skipped until the next frame.
- Errors are caught and written to the log. **5 errors within a minute turn
  the plugin off** (you can switch it on again).
- Scripts are loaded as source text only; precompiled bytecode is refused.

## Security model (summary)

Each plugin gets its own Luau VM with `Lua::sandbox(true)` (read-only
built-ins and API), no `debug` library, no way to load bytecode or files
outside its folder, a memory cap and an interrupt-based time budget. The
API has no network, file system, process, token or account functions. The
only effects a plugin has are: text it proposes for the message box (you
send it), how messages are displayed on your PC, widgets RustCord draws for
it (images only from Discord's CDN, so image URLs can't be used to leak
data to other servers), local notices, and its own storage and log.

## Integration notes (for RustCord developers)

`src/plugins/mod.rs` documents where `PluginHost` hooks into `app.rs`:
`update` each frame, `on_ready` / `on_message` / `on_message_update` /
`on_channel_open` from events, `transform_display` + `display_generation`
in the message pane, `parse_command` / `run_command` in the composer,
`draw_panels` for a plugins tile, `drain_notifications` for toasts,
`settings_ui` for Settings → Plugins, and `save` from `App::save`.
