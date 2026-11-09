//! Installing a plugin from a folder or a `.zip`: copy / extract it into a
//! staging folder (inside the plugins dir), with strict checks.
//!
//! Zip entries with `..`, absolute paths, drive letters or symlinks reject
//! the whole archive; the total size and file count are capped (actual
//! bytes are counted, so lying headers / zip bombs stop at the cap).

use std::fs;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

use super::manifest::{MAX_MANIFEST_BYTES, Manifest};

/// Largest plugin (all files together).
pub const MAX_INSTALL_BYTES: u64 = 8 * 1024 * 1024;
/// Most files (and folders) in one plugin.
pub const MAX_FILES: usize = 512;
/// Deepest folder nesting.
const MAX_DEPTH: usize = 8;

/// Copy (folder) or extract (`.zip`) `src` into the empty folder `staging`.
/// Returns the folder holding `plugin.toml` and its manifest: `staging`
/// itself, or its only subfolder (zips usually wrap everything in one).
pub fn stage(src: &Path, staging: &Path) -> Result<(PathBuf, Manifest), String> {
    fs::create_dir_all(staging).map_err(|e| format!("couldn't create a folder: {e}"))?;
    let meta = fs::symlink_metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
    if meta.is_dir() {
        let mut budget = Budget::default();
        copy_dir(src, staging, 0, &mut budget)?;
    } else if meta.is_file()
        && src
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        let file = fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        extract_zip(file, staging)?;
    } else {
        return Err("Choose a plugin folder or a .zip file".into());
    }
    let root = plugin_root(staging)?;
    let manifest = read_manifest(&root)?;
    Ok((root, manifest))
}

/// Read and validate `<dir>/plugin.toml`, and check the entry file exists.
pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("plugin.toml");
    let meta = fs::symlink_metadata(&path).map_err(|_| "No plugin.toml found".to_string())?;
    if !meta.is_file() || meta.len() as usize > MAX_MANIFEST_BYTES {
        return Err("plugin.toml isn't a normal (small) file".into());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("plugin.toml: {e}"))?;
    let manifest = Manifest::parse(&text)?;
    let entry = dir.join(&manifest.entry);
    if !fs::symlink_metadata(&entry).is_ok_and(|m| m.is_file()) {
        return Err(format!("Entry file {} is missing", manifest.entry));
    }
    Ok(manifest)
}

/// `staging` if it has plugin.toml, else its only subfolder that does.
fn plugin_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join("plugin.toml").is_file() {
        return Ok(staging.to_path_buf());
    }
    let dirs: Vec<PathBuf> = fs::read_dir(staging)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name() != "__MACOSX")
        .map(|e| e.path())
        .collect();
    match dirs.as_slice() {
        [one] if one.join("plugin.toml").is_file() => Ok(one.clone()),
        _ => Err("No plugin.toml found (it must be at the top of the folder or zip)".into()),
    }
}

#[derive(Default)]
struct Budget {
    bytes: u64,
    files: usize,
}

impl Budget {
    fn add_file(&mut self) -> Result<(), String> {
        self.files += 1;
        if self.files > MAX_FILES {
            return Err(format!("Too many files (max {MAX_FILES})"));
        }
        Ok(())
    }

    fn remaining(&self) -> u64 {
        MAX_INSTALL_BYTES.saturating_sub(self.bytes)
    }
}

fn too_big() -> String {
    format!(
        "Plugin is too big (max {} MB)",
        MAX_INSTALL_BYTES / 1024 / 1024
    )
}

/// Recursive copy that refuses symlinks and respects the budget.
fn copy_dir(src: &Path, dest: &Path, depth: usize, budget: &mut Budget) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("Folders are nested too deeply".into());
    }
    let entries = fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let to = dest.join(entry.file_name());
        budget.add_file()?;
        if ty.is_symlink() {
            return Err(format!(
                "{} is a symlink; plugins can't contain links",
                entry.path().display()
            ));
        } else if ty.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            fs::create_dir(&to).map_err(|e| e.to_string())?;
            copy_dir(&entry.path(), &to, depth + 1, budget)?;
        } else if ty.is_file() {
            let mut input = fs::File::open(entry.path()).map_err(|e| e.to_string())?;
            write_capped(&mut input, &to, budget)?;
        }
    }
    Ok(())
}

/// Copy at most the remaining budget into a new file at `to`.
fn write_capped(input: &mut impl Read, to: &Path, budget: &mut Budget) -> Result<(), String> {
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    let limit = budget.remaining();
    let copied = std::io::copy(&mut input.take(limit + 1), &mut out).map_err(|e| e.to_string())?;
    if copied > limit {
        return Err(too_big());
    }
    budget.bytes += copied;
    Ok(())
}

/// A zip entry name that is a plain relative path (no `..`, no root, no
/// drive letter, no NUL).
fn safe_relative(name: &str) -> Option<PathBuf> {
    if name.is_empty()
        || name.contains('\0')
        || name.contains(':')
        || name.starts_with('/')
        || name.starts_with('\\')
    {
        return None;
    }
    let mut out = PathBuf::new();
    for seg in name.split(['/', '\\']) {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => out.push(s),
        }
    }
    // Belt and braces: only normal components.
    if out.as_os_str().is_empty()
        || !out.components().all(|c| matches!(c, Component::Normal(_)))
        || out.components().count() > MAX_DEPTH + 1
    {
        return None;
    }
    Some(out)
}

/// Extract a zip into `dest` (which should be empty). Any unsafe entry
/// rejects the archive.
pub fn extract_zip(reader: impl Read + Seek, dest: &Path) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("Not a valid zip file: {e}"))?;
    if archive.len() > MAX_FILES {
        return Err(format!("Too many files (max {MAX_FILES})"));
    }
    let mut budget = Budget::default();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Broken zip entry: {e}"))?;
        let name = file
            .name()
            .map_err(|e| format!("Broken zip entry name: {e}"))?
            .into_owned();
        if file.is_symlink() {
            return Err(format!(
                "\"{name}\" is a symlink; plugins can't contain links"
            ));
        }
        if file.encrypted() {
            return Err("Encrypted zips aren't supported".into());
        }
        let (Some(rel), Some(_)) = (safe_relative(&name), file.enclosed_name()) else {
            return Err(format!("Unsafe path in zip: \"{name}\""));
        };
        if file.size() > budget.remaining() {
            return Err(too_big());
        }
        budget.add_file()?;
        let to = dest.join(&rel);
        if file.is_dir() {
            fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        write_capped(&mut file, &to, &mut budget)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rc-install-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const MANIFEST: &[u8] = b"id = \"zipped\"\nname = \"Zipped\"\nversion = \"1.0\"\n";

    #[test]
    fn extracts_good_zip_with_wrapper_folder() {
        let dir = tmp("good");
        let zip_path = dir.join("p.zip");
        fs::write(
            &zip_path,
            zip_with(&[
                ("zipped/plugin.toml", MANIFEST),
                ("zipped/main.luau", b"rustcord.log('hi')"),
            ]),
        )
        .unwrap();
        let (root, m) = stage(&zip_path, &dir.join("stage")).unwrap();
        assert_eq!(m.id, "zipped");
        assert!(root.ends_with("zipped"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_path_traversal() {
        for evil in [
            "../evil.luau",
            "a/../../evil.luau",
            "/etc/evil",
            "\\windows\\evil",
            "C:/evil",
            "a\\..\\..\\evil",
        ] {
            let dir = tmp("trav");
            let data = zip_with(&[("plugin.toml", MANIFEST), (evil, b"x")]);
            let err = extract_zip(Cursor::new(data), &dir.join("out")).unwrap_err();
            assert!(err.contains("Unsafe path"), "{evil}: {err}");
            assert!(!dir.join("evil.luau").exists());
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn rejects_symlinks() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        w.add_symlink("link", "/etc/passwd", opts).unwrap();
        let data = w.finish().unwrap().into_inner();
        let dir = tmp("sym");
        let err = extract_zip(Cursor::new(data), &dir).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn caps_total_size() {
        let big = vec![0u8; MAX_INSTALL_BYTES as usize + 10];
        let data = zip_with(&[("plugin.toml", MANIFEST), ("big.bin", &big)]);
        let dir = tmp("big");
        let err = extract_zip(Cursor::new(data), &dir).unwrap_err();
        assert!(err.contains("too big"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn folder_copy_and_missing_manifest() {
        let dir = tmp("folder");
        let src = dir.join("src");
        fs::create_dir_all(src.join("lib")).unwrap();
        fs::write(src.join("plugin.toml"), MANIFEST).unwrap();
        fs::write(src.join("main.luau"), "").unwrap();
        fs::write(src.join("lib/util.luau"), "").unwrap();
        let (root, m) = stage(&src, &dir.join("stage")).unwrap();
        assert_eq!(m.id, "zipped");
        assert!(root.join("lib/util.luau").is_file());

        let empty = dir.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(
            stage(&empty, &dir.join("stage2"))
                .unwrap_err()
                .contains("plugin.toml")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn folder_with_symlink_is_rejected() {
        let dir = tmp("folder-sym");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("plugin.toml"), MANIFEST).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("main.luau")).unwrap();
        let err = stage(&src, &dir.join("stage")).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }
}
