//! One plugin's Luau VM: sandbox, limits and the `rustcord` API.
//!
//! Security model (see docs/PLUGINS.md):
//! - Only safe Luau libraries are loaded (no `debug`); `require`,
//!   `loadstring`, `getfenv` / `setfenv` are removed, and `require` is
//!   replaced by a loader that only reads `.luau` / `.lua` files from the
//!   plugin's own folder. `Lua::sandbox(true)` makes the built-in libraries
//!   and the `rustcord` API read-only.
//! - Source is always loaded as text, so precompiled bytecode is refused.
//! - Memory is capped per VM ([`MEMORY_LIMIT`]) and every call into Lua has
//!   a time budget ([`CALL_BUDGET`]) enforced by Luau's interrupt callback.
//! - The API has no network, file, token or account access. The only ways
//!   out are: text for the message box (the user sends it), locally shown
//!   message text, UI drawn by Rust from widget specs, local notices,
//!   the plugin's own storage and its log.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Table, Value, VmState};

use super::manifest::{Manifest, Permission};
use super::storage::Storage;

/// Memory cap per plugin VM.
pub const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
/// Longest a single call into a plugin may run.
pub const CALL_BUDGET: Duration = Duration::from_millis(50);
/// Largest accepted script file.
const MAX_SOURCE_BYTES: u64 = 512 * 1024;
const MAX_COMMANDS: usize = 32;
const MAX_PANELS: usize = 8;
const MAX_HANDLERS: usize = 16;
const MAX_WIDGETS: usize = 256;
/// Longest text a plugin may hand back (composer draft, display text).
pub const MAX_TEXT: usize = 4000;
const MAX_LABEL: usize = 2000;
const LOG_LINES: usize = 200;
const MAX_QUEUED_NOTICES: usize = 10;
/// Hosts plugin images may load from (a URL is a network request, so no
/// arbitrary hosts: that would let a plugin leak data in the query string).
const IMAGE_HOSTS: [&str; 2] = [
    "https://cdn.discordapp.com/",
    "https://media.discordapp.net/",
];

/// Events plugins can subscribe to with `rustcord.on`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Ready,
    Message,
    MessageUpdate,
    ChannelOpen,
}

impl Event {
    const ALL: [Event; 4] = [
        Event::Ready,
        Event::Message,
        Event::MessageUpdate,
        Event::ChannelOpen,
    ];

    fn name(self) -> &'static str {
        match self {
            Event::Ready => "ready",
            Event::Message => "message",
            Event::MessageUpdate => "message_update",
            Event::ChannelOpen => "channel_open",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn permission(self) -> Option<Permission> {
        match self {
            Event::Message | Event::MessageUpdate => Some(Permission::ReadMessages),
            Event::Ready | Event::ChannelOpen => None,
        }
    }
}

/// One line of a plugin's log.
#[derive(Clone)]
pub struct LogLine {
    pub time: String,
    pub text: String,
    pub error: bool,
}

/// Per-plugin log (kept across reloads).
#[derive(Default)]
pub struct LogRing {
    pub lines: VecDeque<LogLine>,
}

impl LogRing {
    pub fn push(&mut self, text: impl Into<String>, error: bool) {
        if self.lines.len() >= LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            text: truncate(text.into(), 1000),
            error,
        });
    }
}

/// A widget spec built by Lua each frame and drawn by Rust.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    Heading(String),
    Label(String),
    /// Small muted text.
    Small(String),
    Separator,
    /// `cb` indexes the callbacks returned with the widgets.
    Button {
        text: String,
        cb: usize,
    },
    /// Text box bound to a storage key (string).
    TextBox {
        key: String,
        hint: String,
        multiline: bool,
    },
    /// Checkbox bound to a storage key (bool).
    Checkbox {
        key: String,
        text: String,
    },
    Image {
        url: String,
        size: [f32; 2],
    },
}

/// Which UI function to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiTarget {
    Panel(usize),
    Settings,
}

pub struct Command {
    pub name: String,
    pub description: String,
    f: Function,
}

struct Panel {
    title: String,
    f: Function,
}

/// What the plugin registered, plus its outputs waiting for the host.
struct Hooks {
    perms: Vec<Permission>,
    events: [Vec<Function>; 4],
    commands: Vec<Command>,
    display: Option<Function>,
    panels: Vec<Panel>,
    settings: Option<Function>,
    notices: Vec<String>,
    storage: Storage,
    storage_changed: bool,
    image_urls: Vec<String>,
}

impl Hooks {
    fn need(&self, p: Permission, what: &str) -> mlua::Result<()> {
        if self.perms.contains(&p) {
            Ok(())
        } else {
            Err(mlua::Error::runtime(format!(
                "{what} needs the \"{}\" permission in plugin.toml",
                p.key()
            )))
        }
    }
}

#[derive(Default)]
struct UiBuffer {
    widgets: Vec<Widget>,
    callbacks: Vec<Function>,
}

impl UiBuffer {
    fn push(&mut self, w: Widget) -> mlua::Result<()> {
        if self.widgets.len() >= MAX_WIDGETS {
            return Err(mlua::Error::runtime("too many widgets"));
        }
        self.widgets.push(w);
        Ok(())
    }
}

pub struct Runtime {
    lua: Lua,
    hooks: Rc<RefCell<Hooks>>,
    ui_buf: Rc<RefCell<UiBuffer>>,
    ui_table: Table,
    deadline: Rc<Cell<Option<Instant>>>,
    expired: Rc<Cell<bool>>,
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

/// A short, readable error (cause plus Lua location, no Rust noise).
pub fn clean_error(e: &mlua::Error) -> String {
    let text = e.to_string();
    let text = text.trim().replace("runtime error: ", "");
    truncate(text, 1500)
}

/// Arguments of a UI builder method, without the `ui` table itself when
/// called with `ui:label(...)` instead of `ui.label(...)`.
fn method_args(args: MultiValue) -> VecDeque<Value> {
    let mut v: VecDeque<Value> = args.into_iter().collect();
    if matches!(v.front(), Some(Value::Table(_))) {
        v.pop_front();
    }
    v
}

fn to_text(v: Option<Value>, what: &str) -> mlua::Result<String> {
    match v {
        Some(Value::String(s)) => Ok(s.to_str()?.to_string()),
        Some(Value::Integer(i)) => Ok(i.to_string()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        Some(Value::Boolean(b)) => Ok(b.to_string()),
        _ => Err(mlua::Error::runtime(format!("{what} must be a string"))),
    }
}

fn opt_text(v: Option<Value>) -> String {
    match v {
        Some(Value::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

fn to_num(v: Option<Value>, default: f32) -> f32 {
    match v {
        Some(Value::Integer(i)) => i as f32,
        Some(Value::Number(n)) => n as f32,
        _ => default,
    }
}

fn storage_key(v: Option<Value>) -> mlua::Result<String> {
    let key = to_text(v, "storage key")?;
    if key.is_empty() || key.len() > super::storage::MAX_KEY_LEN {
        return Err(mlua::Error::runtime("storage key must be 1-128 bytes"));
    }
    Ok(key)
}

/// `name` for `require`: plain segments of letters, digits, `-`, `_`.
fn valid_module(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name.split('/').count() <= 4
        && name.split('/').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
}

/// Read a script: must be a regular file (not a link), small, UTF-8.
fn read_script(path: &Path) -> Result<String, String> {
    let meta = fs::symlink_metadata(path).map_err(|_| format!("{} not found", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a normal file", path.display()));
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!("{} is too big", path.display()));
    }
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Runtime {
    /// Create the VM, install the API and run the entry script.
    pub fn load(
        manifest: &Manifest,
        dir: &Path,
        storage: Storage,
        log: Rc<RefCell<LogRing>>,
    ) -> Result<Runtime, String> {
        let source = read_script(&dir.join(&manifest.entry))?;
        let rt = Runtime::new(manifest, dir, storage, log).map_err(|e| clean_error(&e))?;
        rt.guarded(|| {
            rt.lua
                .load(source)
                .set_name(format!("={}/{}", manifest.id, manifest.entry))
                .set_mode(ChunkMode::Text)
                .exec()
        })?;
        Ok(rt)
    }

    fn new(
        manifest: &Manifest,
        dir: &Path,
        storage: Storage,
        log: Rc<RefCell<LogRing>>,
    ) -> mlua::Result<Runtime> {
        let libs = StdLib::COROUTINE
            | StdLib::TABLE
            | StdLib::OS
            | StdLib::STRING
            | StdLib::UTF8
            | StdLib::BIT
            | StdLib::MATH
            | StdLib::BUFFER;
        let lua = Lua::new_with(libs, LuaOptions::new())?;
        lua.set_memory_limit(MEMORY_LIMIT)?;

        // Time budget: checked every 256 interrupts; once over, every
        // interrupt errors so `pcall` can't swallow it and keep looping.
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let expired = Rc::new(Cell::new(false));
        {
            let deadline = deadline.clone();
            let expired = expired.clone();
            let counter = Cell::new(0u32);
            lua.set_interrupt(move |_| {
                if expired.get() {
                    return Err(mlua::Error::runtime("plugin took too long and was stopped"));
                }
                let n = counter.get().wrapping_add(1);
                counter.set(n);
                if n.is_multiple_of(256)
                    && let Some(d) = deadline.get()
                    && Instant::now() > d
                {
                    expired.set(true);
                    return Err(mlua::Error::runtime(format!(
                        "plugin took too long (over {} ms) and was stopped",
                        CALL_BUDGET.as_millis()
                    )));
                }
                Ok(VmState::Continue)
            });
        }

        let globals = lua.globals();
        for name in [
            "require",
            "loadstring",
            "load",
            "dofile",
            "loadfile",
            "getfenv",
            "setfenv",
            "debug",
            "io",
            "package",
        ] {
            globals.raw_set(name, Value::Nil)?;
        }
        // Luau's `os` only has clock/date/difftime/time, but be explicit.
        if let Ok(os) = globals.raw_get::<Table>("os") {
            for name in ["execute", "exit", "getenv", "remove", "rename", "tmpname"] {
                os.raw_set(name, Value::Nil)?;
            }
        }

        let hooks = Rc::new(RefCell::new(Hooks {
            perms: manifest.permissions.clone(),
            events: Default::default(),
            commands: Vec::new(),
            display: None,
            panels: Vec::new(),
            settings: None,
            notices: Vec::new(),
            storage,
            storage_changed: false,
            image_urls: Vec::new(),
        }));
        let ui_buf = Rc::new(RefCell::new(UiBuffer::default()));

        let rc = lua.create_table()?;
        rc.raw_set("version", env!("CARGO_PKG_VERSION"))?;
        let info = lua.create_table()?;
        info.raw_set("id", manifest.id.as_str())?;
        info.raw_set("name", manifest.name.as_str())?;
        info.raw_set("version", manifest.version.as_str())?;
        info.set_readonly(true);
        rc.raw_set("plugin", info)?;

        // rustcord.log(...) and print(...)
        let log_fn = {
            let log = log.clone();
            lua.create_function(move |_, args: MultiValue| {
                let parts: Vec<String> = args
                    .iter()
                    .map(|v| v.to_string().unwrap_or_else(|_| "?".into()))
                    .collect();
                log.borrow_mut().push(parts.join(" "), false);
                Ok(())
            })?
        };
        rc.raw_set("log", log_fn.clone())?;
        globals.raw_set("print", log_fn)?;

        // rustcord.on(event, fn)
        {
            let hooks = hooks.clone();
            rc.raw_set(
                "on",
                lua.create_function(move |_, (event, f): (String, Function)| {
                    let Some(ev) = Event::ALL.into_iter().find(|e| e.name() == event) else {
                        return Err(mlua::Error::runtime(format!(
                            "unknown event \"{event}\" (ready, message, message_update, channel_open)"
                        )));
                    };
                    let mut h = hooks.borrow_mut();
                    if let Some(p) = ev.permission() {
                        h.need(p, &format!("rustcord.on(\"{event}\")"))?;
                    }
                    let list = &mut h.events[ev.index()];
                    if list.len() >= MAX_HANDLERS {
                        return Err(mlua::Error::runtime("too many event handlers"));
                    }
                    list.push(f);
                    Ok(())
                })?,
            )?;
        }

        // rustcord.command(name, description, fn)
        {
            let hooks = hooks.clone();
            rc.raw_set(
                "command",
                lua.create_function(
                    move |_, (name, description, f): (String, String, Function)| {
                        let mut h = hooks.borrow_mut();
                        h.need(Permission::Commands, "rustcord.command")?;
                        let ok = !name.is_empty()
                            && name.len() <= 32
                            && name.bytes().all(|c| {
                                c.is_ascii_lowercase()
                                    || c.is_ascii_digit()
                                    || c == b'_'
                                    || c == b'-'
                            });
                        if !ok {
                            return Err(mlua::Error::runtime(
                                "command names are 1-32 lowercase letters, digits, - or _",
                            ));
                        }
                        let description = truncate(description, 100);
                        if let Some(c) = h.commands.iter_mut().find(|c| c.name == name) {
                            c.description = description;
                            c.f = f;
                        } else if h.commands.len() >= MAX_COMMANDS {
                            return Err(mlua::Error::runtime("too many commands"));
                        } else {
                            h.commands.push(Command {
                                name,
                                description,
                                f,
                            });
                        }
                        Ok(())
                    },
                )?,
            )?;
        }

        // rustcord.display(fn)
        {
            let hooks = hooks.clone();
            rc.raw_set(
                "display",
                lua.create_function(move |_, f: Function| {
                    let mut h = hooks.borrow_mut();
                    h.need(Permission::MessageDisplay, "rustcord.display")?;
                    h.display = Some(f);
                    Ok(())
                })?,
            )?;
        }

        // rustcord.notify(text)
        {
            let hooks = hooks.clone();
            rc.raw_set(
                "notify",
                lua.create_function(move |_, text: MultiValue| {
                    let mut h = hooks.borrow_mut();
                    h.need(Permission::Notifications, "rustcord.notify")?;
                    let text = to_text(text.into_iter().next(), "notify text")?;
                    if h.notices.len() < MAX_QUEUED_NOTICES {
                        h.notices.push(truncate(text, 300));
                    }
                    Ok(())
                })?,
            )?;
        }

        // rustcord.storage.get / set
        let storage_t = lua.create_table()?;
        {
            let hooks = hooks.clone();
            storage_t.raw_set(
                "get",
                lua.create_function(move |lua, key: MultiValue| {
                    let h = hooks.borrow();
                    h.need(Permission::Storage, "rustcord.storage")?;
                    let key = storage_key(method_args(key).pop_front())?;
                    match h.storage.get(&key) {
                        Some(v) => lua.to_value(v),
                        None => Ok(Value::Nil),
                    }
                })?,
            )?;
        }
        {
            let hooks = hooks.clone();
            storage_t.raw_set(
                "set",
                lua.create_function(move |lua, args: MultiValue| {
                    let mut args = method_args(args);
                    let key = storage_key(args.pop_front())?;
                    let value = args.pop_front().unwrap_or(Value::Nil);
                    let mut h = hooks.borrow_mut();
                    h.need(Permission::Storage, "rustcord.storage")?;
                    let json: serde_json::Value = if value.is_nil() {
                        serde_json::Value::Null
                    } else {
                        lua.from_value(value).map_err(|e| {
                            mlua::Error::runtime(format!("can't store that value: {e}"))
                        })?
                    };
                    h.storage.set(&key, json).map_err(mlua::Error::runtime)?;
                    h.storage_changed = true;
                    Ok(())
                })?,
            )?;
        }
        storage_t.set_readonly(true);
        rc.raw_set("storage", storage_t)?;

        // rustcord.ui.panel / settings
        let ui_t = lua.create_table()?;
        {
            let hooks = hooks.clone();
            ui_t.raw_set(
                "panel",
                lua.create_function(move |_, (title, f): (String, Function)| {
                    let mut h = hooks.borrow_mut();
                    h.need(Permission::Ui, "rustcord.ui.panel")?;
                    let title = truncate(title, 64);
                    if let Some(p) = h.panels.iter_mut().find(|p| p.title == title) {
                        p.f = f;
                    } else if h.panels.len() >= MAX_PANELS {
                        return Err(mlua::Error::runtime("too many panels"));
                    } else {
                        h.panels.push(Panel { title, f });
                    }
                    Ok(())
                })?,
            )?;
        }
        {
            let hooks = hooks.clone();
            ui_t.raw_set(
                "settings",
                lua.create_function(move |_, f: Function| {
                    let mut h = hooks.borrow_mut();
                    h.need(Permission::Ui, "rustcord.ui.settings")?;
                    h.settings = Some(f);
                    Ok(())
                })?,
            )?;
        }
        ui_t.set_readonly(true);
        rc.raw_set("ui", ui_t)?;

        let ui_table = Self::ui_builder(&lua, &hooks, &ui_buf)?;

        rc.set_readonly(true);
        globals.raw_set("rustcord", rc)?;

        // require("name"): only scripts inside this plugin's folder.
        {
            let dir = dir.to_path_buf();
            let id = manifest.id.clone();
            let cache = lua.create_table()?;
            let loading: Rc<RefCell<Vec<String>>> = Rc::default();
            globals.raw_set(
                "require",
                lua.create_function(move |lua, name: String| {
                    if !valid_module(&name) {
                        return Err(mlua::Error::runtime(format!(
                            "require(\"{name}\"): use a module name like \"utils\" or \"lib/utils\" \
                             (files inside the plugin folder only)"
                        )));
                    }
                    let cached: Value = cache.raw_get(name.as_str())?;
                    if !cached.is_nil() {
                        return Ok(cached);
                    }
                    if loading.borrow().contains(&name) {
                        return Err(mlua::Error::runtime(format!(
                            "require(\"{name}\"): circular require"
                        )));
                    }
                    let path = ["luau", "lua"]
                        .iter()
                        .map(|ext| dir.join(format!("{name}.{ext}")))
                        .find(|p| p.is_file())
                        .ok_or_else(|| {
                            mlua::Error::runtime(format!("require(\"{name}\"): module not found"))
                        })?;
                    let src = read_script(&path).map_err(mlua::Error::runtime)?;
                    loading.borrow_mut().push(name.clone());
                    let result = lua
                        .load(src)
                        .set_name(format!("={id}/{name}"))
                        .set_mode(ChunkMode::Text)
                        .call::<Value>(());
                    loading.borrow_mut().retain(|n| n != &name);
                    let mut value = result?;
                    if value.is_nil() {
                        value = Value::Boolean(true);
                    }
                    cache.raw_set(name.as_str(), value.clone())?;
                    Ok(value)
                })?,
            )?;
        }

        lua.sandbox(true)?;

        Ok(Runtime {
            lua,
            hooks,
            ui_buf,
            ui_table,
            deadline,
            expired,
        })
    }

    /// The `ui` object passed to panel / settings functions. Its methods
    /// only append widget specs; Rust draws them after the call.
    fn ui_builder(
        lua: &Lua,
        hooks: &Rc<RefCell<Hooks>>,
        buf: &Rc<RefCell<UiBuffer>>,
    ) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let text_widget = |make: fn(String) -> Widget| {
            let buf = buf.clone();
            lua.create_function(move |_, args: MultiValue| {
                let text = to_text(method_args(args).pop_front(), "text")?;
                buf.borrow_mut().push(make(truncate(text, MAX_LABEL)))
            })
        };
        t.raw_set("heading", text_widget(Widget::Heading)?)?;
        t.raw_set("label", text_widget(Widget::Label)?)?;
        t.raw_set("small", text_widget(Widget::Small)?)?;
        {
            let buf = buf.clone();
            t.raw_set(
                "separator",
                lua.create_function(move |_, _: MultiValue| {
                    buf.borrow_mut().push(Widget::Separator)
                })?,
            )?;
        }
        {
            let buf = buf.clone();
            t.raw_set(
                "button",
                lua.create_function(move |_, args: MultiValue| {
                    let mut a = method_args(args);
                    let text = truncate(to_text(a.pop_front(), "button text")?, 100);
                    let Some(Value::Function(f)) = a.pop_front() else {
                        return Err(mlua::Error::runtime(
                            "ui.button(text, fn): fn must be a function",
                        ));
                    };
                    let mut b = buf.borrow_mut();
                    let cb = b.callbacks.len();
                    b.push(Widget::Button { text, cb })?;
                    b.callbacks.push(f);
                    Ok(())
                })?,
            )?;
        }
        {
            let buf = buf.clone();
            let hooks = hooks.clone();
            t.raw_set(
                "textbox",
                lua.create_function(move |_, args: MultiValue| {
                    hooks
                        .borrow()
                        .need(Permission::Storage, "ui.textbox (bound to storage)")?;
                    let mut a = method_args(args);
                    let key = storage_key(a.pop_front())?;
                    let hint = truncate(opt_text(a.pop_front()), 100);
                    let multiline = matches!(a.pop_front(), Some(Value::Boolean(true)));
                    buf.borrow_mut().push(Widget::TextBox {
                        key,
                        hint,
                        multiline,
                    })
                })?,
            )?;
        }
        {
            let buf = buf.clone();
            let hooks = hooks.clone();
            t.raw_set(
                "checkbox",
                lua.create_function(move |_, args: MultiValue| {
                    hooks
                        .borrow()
                        .need(Permission::Storage, "ui.checkbox (bound to storage)")?;
                    let mut a = method_args(args);
                    let key = storage_key(a.pop_front())?;
                    let text = truncate(to_text(a.pop_front(), "checkbox text")?, 200);
                    buf.borrow_mut().push(Widget::Checkbox { key, text })
                })?,
            )?;
        }
        {
            let buf = buf.clone();
            let hooks = hooks.clone();
            t.raw_set(
                "image",
                lua.create_function(move |_, args: MultiValue| {
                    let mut a = method_args(args);
                    let url = to_text(a.pop_front(), "image url")?;
                    if url.len() > 512 || !IMAGE_HOSTS.iter().any(|h| url.starts_with(h)) {
                        return Err(mlua::Error::runtime(
                            "ui.image: only https://cdn.discordapp.com/ and \
                             https://media.discordapp.net/ images are allowed",
                        ));
                    }
                    let w = to_num(a.pop_front(), 64.0).clamp(8.0, 512.0);
                    let h = to_num(a.pop_front(), w).clamp(8.0, 512.0);
                    let mut hk = hooks.borrow_mut();
                    if !hk.image_urls.contains(&url) {
                        if hk.image_urls.len() >= 64 {
                            return Err(mlua::Error::runtime("too many different images"));
                        }
                        hk.image_urls.push(url.clone());
                    }
                    buf.borrow_mut().push(Widget::Image { url, size: [w, h] })
                })?,
            )?;
        }
        t.set_readonly(true);
        Ok(t)
    }

    /// Run `f` with the time budget armed.
    fn guarded<R>(&self, f: impl FnOnce() -> mlua::Result<R>) -> Result<R, String> {
        self.expired.set(false);
        self.deadline.set(Some(Instant::now() + CALL_BUDGET));
        let r = f();
        self.deadline.set(None);
        self.expired.set(false);
        r.map_err(|e| clean_error(&e))
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn has_handlers(&self, ev: Event) -> bool {
        !self.hooks.borrow().events[ev.index()].is_empty()
    }

    /// Call every handler for `ev` with the value `arg` builds. Returns the
    /// errors (handlers keep running after one fails).
    pub fn fire(&self, ev: Event, arg: impl FnOnce(&Lua) -> mlua::Result<Value>) -> Vec<String> {
        let handlers = self.hooks.borrow().events[ev.index()].clone();
        if handlers.is_empty() {
            return Vec::new();
        }
        let arg = match arg(&self.lua) {
            Ok(a) => a,
            Err(e) => return vec![clean_error(&e)],
        };
        handlers
            .iter()
            .filter_map(|f| self.guarded(|| f.call::<()>(arg.clone())).err())
            .collect()
    }

    /// (name, description) of each registered command.
    pub fn commands(&self) -> Vec<(String, String)> {
        self.hooks
            .borrow()
            .commands
            .iter()
            .map(|c| (c.name.clone(), c.description.clone()))
            .collect()
    }

    /// Run a command. None: this plugin has no such command. Some(Ok(None)):
    /// it ran and returned nil.
    pub fn run_command(
        &self,
        name: &str,
        args: &str,
        ctx: impl FnOnce(&Lua) -> mlua::Result<Value>,
    ) -> Option<Result<Option<String>, String>> {
        let f = self
            .hooks
            .borrow()
            .commands
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.f.clone())?;
        let ctx = match ctx(&self.lua) {
            Ok(c) => c,
            Err(e) => return Some(Err(clean_error(&e))),
        };
        Some(self.guarded(|| {
            f.call::<Option<String>>((args, ctx))
                .map(|r| r.map(|s| truncate(s, MAX_TEXT)))
        }))
    }

    pub fn has_display(&self) -> bool {
        self.hooks.borrow().display.is_some()
    }

    /// Run the display function on a message table. Ok(None): unchanged.
    pub fn display(&self, msg: Table) -> Result<Option<String>, String> {
        let Some(f) = self.hooks.borrow().display.clone() else {
            return Ok(None);
        };
        self.guarded(|| f.call::<Option<String>>(msg))
            .map(|r| r.map(|s| truncate(s, MAX_TEXT)))
    }

    pub fn panel_titles(&self) -> Vec<String> {
        self.hooks
            .borrow()
            .panels
            .iter()
            .map(|p| p.title.clone())
            .collect()
    }

    pub fn has_settings(&self) -> bool {
        self.hooks.borrow().settings.is_some()
    }

    /// Run a panel / settings function and return the widgets it built and
    /// the button callbacks they refer to.
    pub fn build_ui(&self, target: UiTarget) -> Result<(Vec<Widget>, Vec<Function>), String> {
        let f = {
            let h = self.hooks.borrow();
            match target {
                UiTarget::Panel(i) => h.panels.get(i).map(|p| p.f.clone()),
                UiTarget::Settings => h.settings.clone(),
            }
        };
        let Some(f) = f else {
            return Ok((Vec::new(), Vec::new()));
        };
        *self.ui_buf.borrow_mut() = UiBuffer::default();
        let r = self.guarded(|| f.call::<()>(self.ui_table.clone()));
        let buf = std::mem::take(&mut *self.ui_buf.borrow_mut());
        r.map(|()| (buf.widgets, buf.callbacks))
    }

    /// Call a button callback.
    pub fn call(&self, f: &Function) -> Result<(), String> {
        self.guarded(|| f.call::<()>(()))
    }

    pub fn storage_str(&self, key: &str) -> String {
        self.hooks.borrow().storage.get_str(key).to_string()
    }

    pub fn storage_bool(&self, key: &str) -> bool {
        self.hooks.borrow().storage.get_bool(key)
    }

    /// Set a storage key from a bound widget.
    pub fn storage_set(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        let mut h = self.hooks.borrow_mut();
        h.storage.set(key, value)?;
        h.storage_changed = true;
        Ok(())
    }

    /// True once after storage changed (display results may change too).
    pub fn take_storage_changed(&self) -> bool {
        std::mem::take(&mut self.hooks.borrow_mut().storage_changed)
    }

    pub fn flush_storage(&self) -> Result<(), String> {
        self.hooks.borrow_mut().storage.flush()
    }

    pub fn storage_dirty(&self) -> bool {
        self.hooks.borrow().storage.is_dirty()
    }

    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.hooks.borrow_mut().notices)
    }

    pub fn image_urls(&self) -> Vec<String> {
        self.hooks.borrow().image_urls.clone()
    }

    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.flush_storage();
    }
}
