use codex_utils_absolute_path::AbsolutePathBuf;
use include_dir::Dir;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;

use thiserror::Error;

const SYSTEM_SKILLS_DIR: Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/samples");
const SYSTEM_WORKFLOWS_DIR: Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/workflows");

const SYSTEM_SKILLS_DIR_NAME: &str = ".system";
const SKILLS_DIR_NAME: &str = "skills";
const WORKFLOWS_DIR_NAME: &str = "workflows";
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".codex-system-skills.marker";
const SYSTEM_WORKFLOWS_MARKER_FILENAME: &str = ".codex-system-workflows.marker";
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";
const SYSTEM_WORKFLOWS_MARKER_SALT: &str = "v1";

/// Returns the on-disk cache location for embedded system skills from an absolute CODEX_HOME.
pub fn system_cache_root_dir(codex_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    codex_home
        .join(SKILLS_DIR_NAME)
        .join(SYSTEM_SKILLS_DIR_NAME)
}

/// Returns the on-disk cache location for embedded system workflows from an absolute CODEX_HOME.
pub fn system_workflow_cache_root_dir(codex_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    codex_home
        .join(WORKFLOWS_DIR_NAME)
        .join(SYSTEM_SKILLS_DIR_NAME)
}

/// Installs embedded system skills into `CODEX_HOME/skills/.system`.
///
/// Clears any existing system skills directory first and then writes the embedded
/// skills directory into place.
///
/// To avoid doing unnecessary work on every startup, a marker file is written
/// with a fingerprint of the embedded directory. When the marker matches, the
/// install is skipped.
pub fn install_system_skills(codex_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    install_embedded_system_dir(
        &codex_home.join(SKILLS_DIR_NAME),
        &system_cache_root_dir(codex_home),
        &SYSTEM_SKILLS_DIR,
        SYSTEM_SKILLS_MARKER_FILENAME,
        SYSTEM_SKILLS_MARKER_SALT,
    )
}

/// Installs embedded system workflows into `CODEX_HOME/workflows/.system`.
///
/// System workflows use the same cache-and-refresh model as system skills.
pub fn install_system_workflows(codex_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    install_embedded_system_dir(
        &codex_home.join(WORKFLOWS_DIR_NAME),
        &system_workflow_cache_root_dir(codex_home),
        &SYSTEM_WORKFLOWS_DIR,
        SYSTEM_WORKFLOWS_MARKER_FILENAME,
        SYSTEM_WORKFLOWS_MARKER_SALT,
    )
}

fn read_marker(path: &AbsolutePathBuf) -> Result<String, SystemSkillsError> {
    Ok(fs::read_to_string(path.as_path())
        .map_err(|source| SystemSkillsError::io("read bundled resource marker", source))?
        .trim()
        .to_string())
}

fn install_embedded_system_dir(
    root_dir: &AbsolutePathBuf,
    dest_system: &AbsolutePathBuf,
    embedded_dir: &Dir<'_>,
    marker_filename: &str,
    marker_salt: &str,
) -> Result<(), SystemSkillsError> {
    fs::create_dir_all(root_dir.as_path())
        .map_err(|source| SystemSkillsError::io("create bundled resource root dir", source))?;

    let marker_path = dest_system.join(marker_filename);
    let expected_fingerprint = embedded_dir_fingerprint(embedded_dir, marker_salt);
    if dest_system.as_path().is_dir()
        && read_marker(&marker_path).is_ok_and(|marker| marker == expected_fingerprint)
    {
        return Ok(());
    }

    if dest_system.as_path().exists() {
        fs::remove_dir_all(dest_system.as_path()).map_err(|source| {
            SystemSkillsError::io("remove existing bundled resource dir", source)
        })?;
    }

    write_embedded_dir(embedded_dir, dest_system)?;
    fs::write(marker_path.as_path(), format!("{expected_fingerprint}\n"))
        .map_err(|source| SystemSkillsError::io("write bundled resource marker", source))?;
    Ok(())
}

fn embedded_dir_fingerprint(dir: &Dir<'_>, marker_salt: &str) -> String {
    let mut items = Vec::new();
    collect_fingerprint_items(dir, &mut items);
    items.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    let mut hasher = DefaultHasher::new();
    marker_salt.hash(&mut hasher);
    for (path, contents_hash) in items {
        path.hash(&mut hasher);
        contents_hash.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn collect_fingerprint_items(dir: &Dir<'_>, items: &mut Vec<(String, Option<u64>)>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                items.push((subdir.path().to_string_lossy().to_string(), None));
                collect_fingerprint_items(subdir, items);
            }
            include_dir::DirEntry::File(file) => {
                let mut file_hasher = DefaultHasher::new();
                file.contents().hash(&mut file_hasher);
                items.push((
                    file.path().to_string_lossy().to_string(),
                    Some(file_hasher.finish()),
                ));
            }
        }
    }
}

/// Writes the embedded `include_dir::Dir` to disk under `dest`.
///
/// Preserves the embedded directory structure.
fn write_embedded_dir(dir: &Dir<'_>, dest: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    fs::create_dir_all(dest.as_path())
        .map_err(|source| SystemSkillsError::io("create system skills dir", source))?;

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                let subdir_dest = dest.join(subdir.path());
                fs::create_dir_all(subdir_dest.as_path()).map_err(|source| {
                    SystemSkillsError::io("create system skills subdir", source)
                })?;
                write_embedded_dir(subdir, dest)?;
            }
            include_dir::DirEntry::File(file) => {
                let path = dest.join(file.path());
                if let Some(parent) = path.as_path().parent() {
                    fs::create_dir_all(parent).map_err(|source| {
                        SystemSkillsError::io("create system skills file parent", source)
                    })?;
                }
                fs::write(path.as_path(), file.contents())
                    .map_err(|source| SystemSkillsError::io("write system skill file", source))?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum SystemSkillsError {
    #[error("io error while {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SystemSkillsError {
    fn io(action: &'static str, source: std::io::Error) -> Self {
        Self::Io { action, source }
    }
}

#[cfg(test)]
mod tests {
    use super::SYSTEM_SKILLS_DIR;
    use super::SYSTEM_WORKFLOWS_MARKER_FILENAME;
    use super::collect_fingerprint_items;
    use super::install_system_workflows;
    use super::system_workflow_cache_root_dir;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use std::fs;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    #[test]
    fn fingerprint_traverses_nested_entries() {
        let mut items = Vec::new();
        collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
        let mut paths: Vec<String> = items.into_iter().map(|(path, _)| path).collect();
        paths.sort_unstable();

        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/SKILL.md"))
                .is_ok()
        );
        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/scripts/init_skill.py"))
                .is_ok()
        );
    }

    #[test]
    fn install_system_workflows_writes_payload_to_system_cache() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let codex_home = std::env::temp_dir().join(format!(
            "codex-system-workflows-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&codex_home);
        fs::create_dir_all(&codex_home).expect("create temp codex home");
        let codex_home_abs = AbsolutePathBuf::from_absolute_path_checked(codex_home.clone())
            .expect("temp codex home should be absolute");

        install_system_workflows(&codex_home_abs).expect("install system workflows");

        let system_root = system_workflow_cache_root_dir(&codex_home_abs);
        assert_eq!(
            system_root.as_path(),
            codex_home.join("workflows").join(".system")
        );
        let workflow = fs::read_to_string(system_root.join("current-status.js").as_path())
            .expect("read current-status workflow");
        assert!(workflow.starts_with("export const meta = {"));
        assert!(workflow.contains("name: 'current-status'"));
        assert!(
            system_root
                .join(SYSTEM_WORKFLOWS_MARKER_FILENAME)
                .as_path()
                .is_file()
        );

        fs::remove_dir_all(codex_home).expect("remove temp codex home");
    }
}
