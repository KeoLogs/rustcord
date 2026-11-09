//! Plugin system tests: permissions, sandbox, limits, auto-disable,
//! install, and the example plugins.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::json;

use super::runtime::{UiTarget, Widget};
use super::*;
use crate::types::Message;

struct Env {
    root: PathBuf,
    host: PluginHost,
    module_on: bool,
    ctx: egui::Context,
}

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("rc-plugins-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let host = PluginHost::with_dirs(root.join("plugins"), root.join("data"));
        Env {
            root,
            host,
            module_on: true,
            ctx: egui::Context::default(),
        }
    }

    /// Host reading the repo's example plugins (read-only).
    fn examples(name: &str) -> Env {
        let mut env = Env::new(name);
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins");
        env.host = PluginHost::with_dirs(examples, env.root.join("data"));
        env
    }

    /// Write a plugin folder and approve all its permissions.
    fn add(&mut self, id: &str, perms: &[&str], src: &str) {
        let dir = self.root.join("plugins").join(id);
        fs::create_dir_all(&dir).unwrap();
        let perms: Vec<String> = perms.iter().map(|p| format!("\"{p}\"")).collect();
        fs::write(
            dir.join("plugin.toml"),
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0\"\npermissions = [{}]\n",
                perms.join(", ")
            ),
        )
        .unwrap();
        fs::write(dir.join("main.luau"), src).unwrap();
        self.grant(id, perms_of(&perms));
    }

    fn grant(&mut self, id: &str, permissions: Vec<Permission>) {
        self.host.set_grant(PluginGrant {
            id: id.into(),
            permissions,
        });
    }

    fn update(&mut self) {
        self.host.update(&self.ctx, self.module_on);
    }

    fn entry(&self, id: &str) -> &manager::PluginEntry {
        let i = self.host.mgr.index(id).expect("plugin listed");
        &self.host.mgr.entries[i]
    }

    fn load_error(&self, id: &str) -> String {
        self.entry(id).load_error.clone().unwrap_or_default()
    }

    fn run(&mut self, cmd: &str, args: &str) -> Option<CommandOutcome> {
        self.host.run_command(cmd, args, "chan1", Some("guild1"))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn perms_of(quoted: &[String]) -> Vec<Permission> {
    quoted
        .iter()
        .filter_map(|p| Permission::from_key(p.trim_matches('"')))
        .collect()
}

fn message(id: &str, content: &str) -> Message {
    Message::from_json(&json!({
        "id": id,
        "channel_id": "chan1",
        "guild_id": "guild1",
        "content": content,
        "author": { "id": "u1", "username": "sam", "global_name": "Sam" },
    }))
}

fn draft(o: Option<CommandOutcome>) -> String {
    match o {
        Some(CommandOutcome::Draft(s)) => s,
        other => panic!("expected a draft, got {other:?}"),
    }
}

#[test]
fn commands_produce_drafts_only() {
    let mut env = Env::new("cmd");
    env.add(
        "c",
        &["commands"],
        r#"
        rustcord.command("echo", "Echo", function(args, ctx)
            return args .. " in " .. ctx.channel_id .. "/" .. ctx.guild_id
        end)
        rustcord.command("nothing", "Nil", function() return nil end)
        "#,
    );
    env.update();
    assert!(env.entry("c").is_running(), "{}", env.load_error("c"));
    assert_eq!(draft(env.run("echo", "hi")), "hi in chan1/guild1");
    assert_eq!(env.run("nothing", ""), Some(CommandOutcome::NoDraft));
    assert_eq!(env.run("missing", ""), None);
    let cmds = env.host.commands();
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].name, "echo");
    assert_eq!(cmds[0].plugin, "c");
}

#[test]
fn parse_command() {
    assert_eq!(PluginHost::parse_command("/shrug"), Some(("shrug", "")));
    assert_eq!(
        PluginHost::parse_command("  /shrug  hi there "),
        Some(("shrug", "hi there"))
    );
    assert_eq!(PluginHost::parse_command("hi /shrug"), None);
    assert_eq!(PluginHost::parse_command("/Shrug"), None);
    assert_eq!(PluginHost::parse_command("/"), None);
}

#[test]
fn permissions_are_enforced() {
    let mut env = Env::new("perms");
    env.add(
        "nocmd",
        &[],
        r#"rustcord.command("x", "x", function() end)"#,
    );
    env.add("nostore", &["commands"], r#"rustcord.storage.set("a", 1)"#);
    env.add(
        "noread",
        &["ui"],
        r#"rustcord.on("message", function() end)"#,
    );
    env.add("nodisplay", &["ui"], r#"rustcord.display(function() end)"#);
    env.add("nonotify", &[], r#"rustcord.notify("hi")"#);
    env.add(
        "noui",
        &["storage"],
        r#"rustcord.ui.panel("p", function() end)"#,
    );
    env.add(
        "ok",
        &[],
        r#"rustcord.on("ready", function() end) rustcord.on("channel_open", function() end)"#,
    );
    env.update();
    for (id, perm) in [
        ("nocmd", "commands"),
        ("nostore", "storage"),
        ("noread", "read_messages"),
        ("nodisplay", "message_display"),
        ("nonotify", "notifications"),
        ("noui", "ui"),
    ] {
        assert!(!env.entry(id).is_running(), "{id} should fail");
        let err = env.load_error(id);
        assert!(
            err.contains(&format!("\"{perm}\" permission")),
            "{id}: {err}"
        );
    }
    assert!(env.entry("ok").is_running(), "{}", env.load_error("ok"));
}

#[test]
fn manifest_asking_for_more_needs_reapproval() {
    let mut env = Env::new("reapprove");
    env.add(
        "p",
        &["commands"],
        r#"rustcord.command("a", "a", function() return "a" end)"#,
    );
    env.update();
    assert!(env.entry("p").is_running());
    // An update asks for storage too: it stops until approved again.
    fs::write(
        env.root.join("plugins/p/plugin.toml"),
        "id = \"p\"\nname = \"p\"\nversion = \"2.0\"\npermissions = [\"commands\", \"storage\"]\n",
    )
    .unwrap();
    env.host.mgr.reload("p");
    env.update();
    assert!(!env.entry("p").is_running());
    assert_eq!(env.run("a", ""), None);
    env.grant("p", vec![Permission::Commands, Permission::Storage]);
    env.update();
    assert!(env.entry("p").is_running());
}

#[test]
fn sandbox_blocks_dangerous_globals() {
    let mut env = Env::new("sandbox");
    let dir = env.root.join("plugins/s/lib");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("util.luau"),
        "return { twice = function(x) return x * 2 end }",
    )
    .unwrap();
    fs::write(env.root.join("secret.luau"), "return 'leaked'").unwrap();
    env.add(
        "s",
        &["commands"],
        r#"
        assert(io == nil, "io")
        assert(debug == nil, "debug")
        assert(package == nil, "package")
        assert(loadstring == nil, "loadstring")
        assert(load == nil, "load")
        assert(dofile == nil and loadfile == nil, "dofile")
        assert(getfenv == nil and setfenv == nil, "fenv")
        assert(os.execute == nil and os.getenv == nil and os.remove == nil, "os")
        assert(rustcord.token == nil, "token")
        -- require only reaches files inside the plugin folder
        assert(require("lib/util").twice(21) == 42, "require own file")
        assert(not pcall(require, "../secret"), "require ..")
        assert(not pcall(require, "/etc/passwd"), "require abs")
        assert(not pcall(require, "lib/../../secret"), "require traversal")
        -- the API and built-in libraries are read-only
        assert(not pcall(function() rustcord.command = nil end), "rustcord ro")
        assert(not pcall(function() rustcord.storage.get = nil end), "storage ro")
        assert(not pcall(function() string.rep = nil end), "string ro")
        assert(not pcall(function() getmetatable("").__index = {} end), "string mt ro")
        rustcord.command("ok", "", function() return "sandboxed" end)
        "#,
    );
    env.update();
    assert!(env.entry("s").is_running(), "{}", env.load_error("s"));
    assert_eq!(draft(env.run("ok", "")), "sandboxed");
}

#[test]
fn bytecode_is_rejected() {
    let mut env = Env::new("bytecode");
    env.add("b", &[], "");
    let bytecode = mlua::chunk::Compiler::new()
        .compile("rustcord.log('from bytecode')")
        .unwrap();
    fs::write(env.root.join("plugins/b/main.luau"), bytecode).unwrap();
    env.update();
    assert!(!env.entry("b").is_running());
    let err = env.load_error("b");
    assert!(
        err.contains("binary chunk") || err.contains("stream did not contain valid UTF-8"),
        "{err}"
    );
}

#[test]
fn infinite_loops_are_stopped() {
    let mut env = Env::new("timeout");
    env.add("loop", &[], "while true do end");
    env.add(
        "sneaky",
        &[],
        "while true do pcall(function() while true do end end) end",
    );
    env.add(
        "cmdloop",
        &["commands"],
        r#"
        rustcord.command("spin", "", function() while true do end end)
        rustcord.command("fine", "", function() return "still alive" end)
        "#,
    );
    let start = Instant::now();
    env.update();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    for id in ["loop", "sneaky"] {
        assert!(!env.entry(id).is_running());
        assert!(
            env.load_error(id).contains("too long"),
            "{id}: {}",
            env.load_error(id)
        );
    }
    let start = Instant::now();
    match env.run("spin", "") {
        Some(CommandOutcome::Error(e)) => assert!(e.contains("too long"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    // The VM still works after being interrupted.
    assert_eq!(draft(env.run("fine", "")), "still alive");
}

#[test]
fn memory_limit_is_enforced() {
    let mut env = Env::new("memory");
    env.add(
        "hog",
        &[],
        r#"
        local t = {}
        for i = 1, 100000 do t[i] = string.rep("x", 1024 * 1024) .. i end
        "#,
    );
    env.add(
        "bigstr",
        &["commands"],
        r#"
        rustcord.command("grow", "", function() return string.rep("x", 64 * 1024 * 1024) end)
        rustcord.command("ok", "", function() return "ok" end)
        "#,
    );
    env.update();
    assert!(!env.entry("hog").is_running());
    let err = env.load_error("hog");
    assert!(err.to_lowercase().contains("memory"), "{err}");
    match env.run("grow", "") {
        Some(CommandOutcome::Error(e)) => assert!(e.to_lowercase().contains("memory"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(draft(env.run("ok", "")), "ok");
    let i = env.host.mgr.index("bigstr").unwrap();
    let used = env.host.mgr.entries[i]
        .runtime
        .as_ref()
        .unwrap()
        .used_memory();
    assert!(used < runtime::MEMORY_LIMIT, "{used}");
}

#[test]
fn storage_is_capped_and_persisted() {
    let mut env = Env::new("storage");
    env.add(
        "st",
        &["commands", "storage"],
        r#"
        rustcord.command("big", "", function()
            local ok, err = pcall(rustcord.storage.set, "big", string.rep("x", 300 * 1024))
            return if ok then "stored" else tostring(err)
        end)
        rustcord.command("put", "", function(args)
            rustcord.storage.set("note", { text = args, n = 2 })
            return "ok"
        end)
        rustcord.command("get", "", function()
            local v = rustcord.storage.get("note")
            return v and (v.text .. v.n) or "none"
        end)
        "#,
    );
    env.update();
    let big = draft(env.run("big", ""));
    assert!(big.contains("storage is full"), "{big}");
    draft(env.run("put", "hello"));
    assert_eq!(draft(env.run("get", "")), "hello2");
    env.host.save();
    let file = env.root.join("data/st/storage.json");
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved["note"]["text"], "hello");
    // Survives a reload.
    env.host.mgr.reload("st");
    env.update();
    assert_eq!(draft(env.run("get", "")), "hello2");
}

#[test]
fn repeated_errors_auto_disable() {
    let mut env = Env::new("autodisable");
    env.add(
        "bad",
        &["commands"],
        r#"rustcord.command("boom", "", function() error("kaboom") end)"#,
    );
    env.update();
    for n in 0..manager::MAX_ERRORS {
        match env.run("boom", "") {
            Some(CommandOutcome::Error(e)) => assert!(e.contains("kaboom"), "{e}"),
            other => panic!("run {n}: {other:?}"),
        }
    }
    assert!(!env.entry("bad").is_running());
    assert!(env.entry("bad").auto_disabled.is_some());
    env.update();
    // Its approval is withdrawn: the user has to switch it on again.
    assert!(env.host.grant("bad").is_none());
    assert_eq!(env.run("boom", ""), None);
    let log = env.entry("bad").log.borrow();
    assert!(
        log.lines
            .iter()
            .any(|l| l.error && l.text.contains("Turned off"))
    );
}

#[test]
fn events_and_notifications() {
    let mut env = Env::new("events");
    env.add(
        "ev",
        &["read_messages", "commands", "notifications"],
        r#"
        local seen, me, chan = {}, "?", "?"
        rustcord.on("ready", function(u) me = u.name end)
        rustcord.on("channel_open", function(c) chan = c.name end)
        rustcord.on("message", function(m)
            table.insert(seen, m.author.name .. ":" .. m.content .. "@" .. m.channel_id)
            if m.content == "ping" then rustcord.notify("got ping") end
        end)
        rustcord.on("message_update", function(m) table.insert(seen, "edit:" .. m.content) end)
        rustcord.command("seen", "", function()
            return me .. "|" .. chan .. "|" .. table.concat(seen, ",")
        end)
        "#,
    );
    env.update();
    let me = crate::types::User {
        id: "me".into(),
        name: "Leo".into(),
        ..Default::default()
    };
    env.host.on_ready(&me);
    env.host.on_channel_open("chan1", Some("guild1"), "general");
    env.host.on_message(&message("m1", "ping"));
    env.host
        .on_message_update(&crate::types::MessagePatch::from_json(
            &json!({"id": "m1", "channel_id": "chan1", "content": "pong"}),
        ));
    assert_eq!(
        draft(env.run("seen", "")),
        "Leo|general|Sam:ping@chan1,edit:pong"
    );
    let notes = env.host.drain_notifications();
    assert_eq!(
        notes,
        vec![PluginNotification {
            plugin: "ev".into(),
            text: "got ping".into()
        }]
    );
    assert!(env.host.drain_notifications().is_empty());
}

#[test]
fn module_off_loads_nothing() {
    let mut env = Env::new("off");
    env.add(
        "c",
        &["commands"],
        r#"rustcord.command("a", "", function() return "a" end)"#,
    );
    env.module_on = false;
    env.update();
    assert!(!env.host.is_active());
    assert!(env.host.mgr.entries.is_empty(), "nothing scanned");
    assert_eq!(env.run("a", ""), None);
    env.module_on = true;
    env.update();
    assert_eq!(draft(env.run("a", "")), "a");
    // Switching off again drops the VMs.
    env.module_on = false;
    env.update();
    assert!(env.host.mgr.running().next().is_none());
    assert!(env.host.commands().is_empty());
}

#[test]
fn install_from_zip_and_uninstall() {
    use std::io::Write;
    let mut env = Env::new("install");
    let zip_path = env.root.join("greet.zip");
    {
        let mut w = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("greet/plugin.toml", opts).unwrap();
        w.write_all(
            b"id = \"greet\"\nname = \"Greet\"\nversion = \"1.0\"\npermissions = [\"commands\"]\n",
        )
        .unwrap();
        w.start_file("greet/main.luau", opts).unwrap();
        w.write_all(b"rustcord.command('hi', '', function() return 'hello!' end)")
            .unwrap();
        w.finish().unwrap();
    }
    env.update();
    env.host.begin_install(&zip_path);
    let pending = env.host.ui.pending.take().expect("staged");
    assert_eq!(pending.manifest.permissions, vec![Permission::Commands]);
    let msg = env.host.finish_install(pending, true);
    assert_eq!(msg, "Installed Greet");
    env.update();
    assert_eq!(draft(env.run("hi", "")), "hello!");
    assert!(env.root.join("plugins/greet/plugin.toml").is_file());
    // No staging leftovers.
    let leftovers = fs::read_dir(env.root.join("plugins"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().starts_with('.'))
        })
        .count();
    assert_eq!(leftovers, 0);

    env.host.mgr.uninstall("greet").unwrap();
    assert!(!env.root.join("plugins/greet").exists());
    assert_eq!(env.run("hi", ""), None);
}

#[test]
fn example_plugins_work() {
    let mut env = Env::examples("examples");
    env.grant("hello", vec![Permission::Commands]);
    env.grant(
        "word-filter",
        vec![
            Permission::MessageDisplay,
            Permission::Ui,
            Permission::Storage,
        ],
    );
    env.grant("channel-notes", vec![Permission::Ui, Permission::Storage]);
    env.update();
    for id in ["hello", "word-filter", "channel-notes"] {
        assert!(env.entry(id).is_running(), "{id}: {}", env.load_error(id));
    }

    // hello
    assert_eq!(draft(env.run("shrug", "")), "¯\\_(ツ)_/¯");
    assert_eq!(draft(env.run("shrug", "oh well")), "oh well ¯\\_(ツ)_/¯");
    let time = draft(env.run("time", ""));
    assert!(time.starts_with("<t:") && time.ends_with(":t>"), "{time}");

    // word-filter: nothing configured → unchanged.
    let msg = message("w1", "Well DARN it, darned thing");
    assert_eq!(env.host.transform_display(&msg), None);
    let gen0 = env.host.display_generation();
    let i = env.host.mgr.index("word-filter").unwrap();
    let rt = env.host.mgr.entries[i].runtime.as_ref().unwrap();
    rt.storage_set("words", json!("darn, it")).unwrap();
    env.update();
    assert!(
        env.host.display_generation() > gen0,
        "storage change invalidates"
    );
    assert_eq!(
        env.host.transform_display(&msg).as_deref(),
        Some("Well ||DARN|| ||it||, darned thing")
    );
    let i = env.host.mgr.index("word-filter").unwrap();
    let rt = env.host.mgr.entries[i].runtime.as_ref().unwrap();
    rt.storage_set("replace", json!(true)).unwrap();
    let (widgets, _) = rt.build_ui(UiTarget::Settings).unwrap();
    assert!(
        widgets
            .iter()
            .any(|w| matches!(w, Widget::TextBox { key, .. } if key == "words"))
    );
    env.update();
    assert_eq!(
        env.host.transform_display(&msg).as_deref(),
        Some("Well •••• ••, darned thing")
    );

    // channel-notes
    let panels = env.host.panels();
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0].title, "Channel notes");
    env.host.on_channel_open("123", Some("g"), "general");
    let i = env.host.mgr.index("channel-notes").unwrap();
    let rt = env.host.mgr.entries[i].runtime.as_ref().unwrap();
    let (widgets, callbacks) = rt.build_ui(UiTarget::Panel(0)).unwrap();
    assert_eq!(widgets[0], Widget::Heading("#general".into()));
    assert!(
        matches!(&widgets[1], Widget::TextBox { key, multiline: true, .. } if key == "note:123")
    );
    rt.storage_set("note:123", json!("remember")).unwrap();
    assert_eq!(rt.storage_str("note:123"), "remember");
    // "Clear" button
    let Widget::Button { cb, .. } = &widgets[2] else {
        panic!("{widgets:?}")
    };
    rt.call(&callbacks[*cb]).unwrap();
    assert_eq!(rt.storage_str("note:123"), "");

    // Panels render without errors.
    let mut drawn = false;
    let mut out = env.ctx.run_ui(Default::default(), |ui| {
        let pal = crate::theme::ThemeChoice::default().palette();
        env.host.draw_panels(ui, &pal);
        drawn = true;
    });
    out.textures_delta.clear();
    assert!(drawn);
    assert!(env.entry("channel-notes").is_running());
}

#[test]
fn ui_images_only_from_discord_cdn() {
    let mut env = Env::new("images");
    env.add(
        "img",
        &["ui"],
        r#"
        rustcord.ui.panel("p", function(ui)
            ui:image("https://cdn.discordapp.com/emojis/1.png", 32)
            assert(not pcall(ui.image, "https://evil.example/?leak=1"))
            assert(not pcall(ui.image, "http://cdn.discordapp.com/x.png"))
            ui:label("done")
        end)
        "#,
    );
    env.update();
    let i = env.host.mgr.index("img").unwrap();
    let rt = env.host.mgr.entries[i].runtime.as_ref().unwrap();
    let (widgets, _) = rt.build_ui(UiTarget::Panel(0)).unwrap();
    assert_eq!(widgets.len(), 2, "{widgets:?}");
    assert!(matches!(&widgets[0], Widget::Image { size, .. } if size == &[32.0, 32.0]));
}

#[test]
fn settings_page_renders() {
    let mut env = Env::examples("settings-ui");
    env.grant("hello", vec![Permission::Commands]);
    env.update();
    let pal = crate::theme::ThemeChoice::default().palette();
    // A staged install (approval dialog) and the log viewer open together.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/channel-notes");
    let staging_env = Env::new("settings-ui-stage");
    let pending = staging_env.host.mgr.stage_install(&src).unwrap();
    assert_eq!(pending.manifest.id, "channel-notes");
    env.host.ui.pending = Some(pending);
    env.host.ui.log_view = Some("hello".into());
    env.host.ui.confirm_uninstall = Some("word-filter".into());
    env.host.ui.approve_enable = Some("word-filter".into());
    for _ in 0..2 {
        let mut out = env.ctx.run_ui(Default::default(), |ui| {
            let action = env.host.settings_ui(ui, env.module_on, &pal);
            assert_eq!(action, None);
        });
        out.textures_delta.clear();
    }
    assert!(env.host.ui.pending.is_some(), "dialog waits for a choice");
    let pending = env.host.ui.pending.take().unwrap();
    staging_env.host.mgr.cancel_install(pending);
    // Module off: the page only offers the switch.
    env.module_on = false;
    env.update();
    let mut out = env.ctx.run_ui(Default::default(), |ui| {
        env.host.settings_ui(ui, env.module_on, &pal);
    });
    out.textures_delta.clear();
    assert!(!env.host.is_active());
}

#[test]
fn enabled_list_persists_and_needs_the_module() {
    let mut env = Env::new("grants");
    env.add(
        "g",
        &["commands"],
        r#"rustcord.command("g", "", function() return "hi" end)"#,
    );
    env.update();
    assert!(env.entry("g").is_running());
    // A fresh host (next app start) reads the saved list.
    let mut host = PluginHost::with_dirs(env.root.join("plugins"), env.root.join("data"));
    host.update(&env.ctx, false);
    assert!(host.grants.is_none(), "module off: nothing read");
    host.update(&env.ctx, true);
    assert!(host.grant("g").is_some());
    assert_eq!(
        host.run_command("g", "", "1", None),
        Some(CommandOutcome::Draft("hi".into()))
    );
    host.revoke("g");
    host.update(&env.ctx, true);
    assert!(host.run_command("g", "", "1", None).is_none());
    let mut host = PluginHost::with_dirs(env.root.join("plugins"), env.root.join("data"));
    assert!(host.grant("g").is_none());
}
