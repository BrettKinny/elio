//! Reversible per-user FileChooser portal routing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

const FILECHOOSER: &str = "org.freedesktop.impl.portal.FileChooser";
const MODE: u32 = 0o600;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct State {
    records: Vec<Record>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    phase: Phase,
    logical: PathBuf,
    target: PathBuf,
    created: bool,
    previous: Option<String>,
    before: String,
    after: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Committed,
}

struct Paths {
    config_home: PathBuf,
    state: PathBuf,
    search: Vec<PathBuf>,
    desktops: Vec<String>,
}
#[derive(Debug)]
struct Target {
    logical: PathBuf,
    target: PathBuf,
    source: Option<PathBuf>,
    created: bool,
}

pub(crate) fn enable() -> Result<()> {
    enable_at(&Paths::from_env()?)
}
pub(crate) fn disable() -> Result<bool> {
    disable_at(&Paths::from_env()?)
}

pub(crate) fn status() -> Result<(Option<PathBuf>, Option<String>, &'static str)> {
    let paths = Paths::from_env()?;
    let effective = effective(&paths)?;
    let value = effective
        .as_ref()
        .map(|path| source(path))
        .transpose()?
        .map(|text| value(&text))
        .transpose()?
        .flatten();
    let state = match state(&paths.state)? {
        None => "disabled",
        Some(state)
            if state
                .records
                .iter()
                .all(|record| record.phase == Phase::Committed) =>
        {
            "managed"
        }
        Some(_) => "recovery required",
    };
    Ok((effective, value, state))
}

/// Tests the effective explicit FileChooser route, falling back to `default`.
pub(crate) fn elio_referenced() -> Result<bool> {
    let paths = Paths::from_env()?;
    let Some(path) = effective(&paths)? else {
        return Ok(false);
    };
    let text = source(&path)?;
    let route = value(&text)?.or(default(&text)?);
    Ok(route.is_some_and(|route| route.split(';').any(|backend| backend.trim() == "elio")))
}

fn enable_at(paths: &Paths) -> Result<()> {
    let _lock = Lock::new(&paths.state)?;
    let target = target(paths)?;
    reconcile(paths)?;
    let mut state = state(&paths.state)?.unwrap_or(State {
        records: Vec::new(),
    });
    if let Some(record) = state
        .records
        .iter()
        .find(|record| record.logical == target.logical)
    {
        let (current, _) = owned(&record.target)?;
        if hash(&current) == record.after || value(&current)?.as_deref() == Some("elio") {
            return Ok(());
        }
        anyhow::bail!(
            "error: managed FileChooser routing changed after enable; refusing to overwrite it"
        )
    }
    let before = match &target.source {
        Some(path) if path == &target.target => owned(path)?.0,
        Some(path) => source(path)?,
        None => String::new(),
    };
    let (after, previous) = replace(&before, "elio")?;
    if previous.as_deref() == Some("elio") {
        anyhow::bail!(
            "error: {} already selects elio but is not managed by elio",
            target.logical.display()
        )
    }
    let record = Record {
        phase: Phase::Prepared,
        logical: target.logical,
        target: target.target,
        created: target.created,
        previous,
        before: hash(&before),
        after: hash(&after),
    };
    state.records.push(record.clone());
    write_state(&paths.state, &state)?;
    atomic_checked(
        &record.target,
        after.as_bytes(),
        mode(&record.target)?,
        if record.created {
            Expected::Missing
        } else {
            Expected::Hash(&record.before)
        },
    )?;
    if hash(&owned(&record.target)?.0) != record.after {
        anyhow::bail!("error: portal routing write verification failed")
    }
    write_state(
        &paths.state,
        &State {
            records: state
                .records
                .into_iter()
                .map(|saved| Record {
                    phase: if saved.logical == record.logical {
                        Phase::Committed
                    } else {
                        saved.phase
                    },
                    ..saved
                })
                .collect(),
        },
    )
}

fn disable_at(paths: &Paths) -> Result<bool> {
    let _lock = Lock::new(&paths.state)?;
    reconcile(paths)?;
    let Some(state) = state(&paths.state)? else {
        return Ok(false);
    };
    let mut restored_any = false;
    for record in state.records {
        let (current, _) = match owned(&record.target) {
            Ok(current) => current,
            Err(error) if is_missing(&error) => continue,
            Err(error) => return Err(error),
        };
        if value(&current)?.as_deref() != Some("elio") {
            continue;
        }
        if record.created && hash(&current) == record.after {
            recheck(&record.target, Expected::Hash(&hash(&current)))?;
            fs::remove_file(&record.target)
                .with_context(|| format!("error: failed to remove {}", record.target.display()))?;
        } else {
            let restored = restore(&current, record.previous.as_deref())?;
            atomic_checked(
                &record.target,
                restored.as_bytes(),
                mode(&record.target)?,
                Expected::Hash(&hash(&current)),
            )?;
        }
        restored_any = true;
    }
    remove_state(&paths.state)?;
    Ok(restored_any)
}

fn reconcile(paths: &Paths) -> Result<()> {
    let Some(mut state) = state(&paths.state)? else {
        return Ok(());
    };
    let mut changed = false;
    let mut discard = Vec::with_capacity(state.records.len());
    for record in &mut state.records {
        if record.phase == Phase::Committed {
            discard.push(false);
            continue;
        }
        let current = match owned(&record.target) {
            Ok((text, _)) => Some(hash(&text)),
            Err(error) if record.created && is_missing(&error) => {
                changed = true;
                discard.push(true);
                continue;
            }
            Err(_) => None,
        };
        if current.as_deref() == Some(&record.before) {
            changed = true;
            discard.push(true);
        } else if current.as_deref() == Some(&record.after) {
            record.phase = Phase::Committed;
            changed = true;
            discard.push(false);
        } else {
            anyhow::bail!(
                "error: incomplete portal routing update; target changed, refusing recovery"
            )
        }
    }
    state.records = state
        .records
        .into_iter()
        .zip(discard)
        .filter_map(|(record, discard)| (!discard).then_some(record))
        .collect();
    if state.records.is_empty() {
        remove_state(&paths.state)
    } else if changed {
        write_state(&paths.state, &state)
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/routing.rs"]
mod tests;

impl Paths {
    fn from_env() -> Result<Self> {
        let home = absolute("HOME", None)?;
        let config_home = absolute("XDG_CONFIG_HOME", Some(home.join(".config")))?;
        let state_home = absolute("XDG_STATE_HOME", Some(home.join(".local/state")))?;
        let data_home = absolute("XDG_DATA_HOME", Some(home.join(".local/share")))?;
        let mut search = vec![config_home.clone()];
        extend(&mut search, dirs("XDG_CONFIG_DIRS", "/etc/xdg")?);
        unique(
            &mut search,
            PathBuf::from(option_env!("ELIO_PORTAL_SYSCONFDIR").unwrap_or("/etc")),
        );
        unique(&mut search, data_home);
        extend(
            &mut search,
            dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share")?,
        );
        unique(
            &mut search,
            PathBuf::from(option_env!("ELIO_PORTAL_DATADIR").unwrap_or("/usr/share")),
        );
        let desktops = env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter_map(|part| {
                let part = part.trim();
                (!part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
                .then(|| part.to_ascii_lowercase())
            })
            .collect();
        Ok(Self {
            config_home,
            state: state_home.join("elio/portal-routing.json"),
            search,
            desktops,
        })
    }
}

fn effective(paths: &Paths) -> Result<Option<PathBuf>> {
    for base in &paths.search {
        for name in names(&paths.desktops) {
            let candidate = base.join("xdg-desktop-portal").join(name);
            if fs::symlink_metadata(&candidate).is_ok() {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

fn target(paths: &Paths) -> Result<Target> {
    let source_path = effective(paths)?;
    let name = source_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                names(&paths.desktops)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "portals.conf".to_string()),
            )
        });
    let logical = paths.config_home.join("xdg-desktop-portal").join(name);
    match fs::symlink_metadata(&logical) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = fs::canonicalize(&logical).with_context(|| {
                format!(
                    "error: dangling portal config symlink {}",
                    logical.display()
                )
            })?;
            regular_owned(&target)?;
            if !writable(&target) {
                anyhow::bail!(
                    "error: selected user portal config {} is a declarative/read-only symlink; refuse to replace it",
                    logical.display()
                )
            }
            Ok(Target {
                logical,
                source: Some(target.clone()),
                target,
                created: false,
            })
        }
        Ok(_) => {
            regular_owned(&logical)?;
            Ok(Target {
                logical: logical.clone(),
                source: Some(logical.clone()),
                target: logical,
                created: false,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Target {
            logical: logical.clone(),
            target: logical,
            source: source_path,
            created: true,
        }),
        Err(error) => Err(error.into()),
    }
}

fn names(desktops: &[String]) -> Vec<String> {
    desktops
        .iter()
        .map(|desktop| format!("{desktop}-portals.conf"))
        .chain(std::iter::once("portals.conf".to_string()))
        .collect()
}

fn source(path: &Path) -> Result<String> {
    if !fs::metadata(path)?.is_file() {
        anyhow::bail!(
            "error: portal config source is not a regular file: {}",
            path.display()
        )
    }
    fs::read_to_string(path).with_context(|| format!("error: failed to read {}", path.display()))
}
fn owned(path: &Path) -> Result<(String, u32)> {
    regular_owned(path)?;
    Ok((source(path)?, fs::metadata(path)?.mode() & 0o777))
}
fn regular_owned(path: &Path) -> Result<()> {
    let meta = fs::metadata(path)
        .with_context(|| format!("error: failed to inspect {}", path.display()))?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } {
        anyhow::bail!("error: unsafe portal config target {}", path.display())
    }
    Ok(())
}
fn writable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.mode() & 0o222 != 0)
        .unwrap_or(false)
}

#[derive(Default)]
struct Parsed {
    section: Option<usize>,
    key: Option<(usize, usize)>,
    value: Option<String>,
}
fn value(text: &str) -> Result<Option<String>> {
    Ok(parse(text, FILECHOOSER)?.value)
}
fn default(text: &str) -> Result<Option<String>> {
    Ok(parse(text, "default")?.value)
}
fn parse(text: &str, wanted: &str) -> Result<Parsed> {
    let mut parsed = Parsed::default();
    let mut preferred = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let end = offset + line.len();
        if let Some(name) = section(line) {
            if name == "preferred" {
                if preferred || parsed.section.is_some() {
                    anyhow::bail!("error: duplicate [preferred] portal config section")
                }
                preferred = true;
            } else if preferred {
                parsed.section = Some(offset);
                preferred = false;
            }
        } else if preferred {
            match assignment(line) {
                Some((key, value)) if key == wanted => {
                    if parsed.key.is_some() {
                        anyhow::bail!("error: duplicate {wanted} portal config key")
                    }
                    parsed.key = Some((offset, end));
                    parsed.value = Some(value.to_string());
                }
                _ => {}
            }
        }
        offset = end;
    }
    if preferred {
        parsed.section = Some(text.len());
    }
    Ok(parsed)
}
fn section(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix('[')?.split_once(']')?.0.into()
}
fn assignment(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.split('#').next()?.trim()))
}

fn replace(text: &str, new: &str) -> Result<(String, Option<String>)> {
    let parsed = parse(text, FILECHOOSER)?;
    let previous = parsed.value.clone();
    let result = match parsed.key {
        Some((start, end)) => format!(
            "{}{}{}",
            &text[..start],
            rewrite(&text[start..end], new),
            &text[end..]
        ),
        None if parsed.section.is_some() => {
            let at = parsed.section.unwrap();
            format!("{}{}{}", &text[..at], inserted(text, new), &text[at..])
        }
        None => {
            let newline = if !text.is_empty() && !text.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            format!("{text}{newline}\n[preferred]\n{FILECHOOSER}={new}\n")
        }
    };
    Ok((result, previous))
}
fn restore(text: &str, previous: Option<&str>) -> Result<String> {
    let parsed = parse(text, FILECHOOSER)?;
    let Some((start, end)) = parsed.key else {
        anyhow::bail!("error: managed FileChooser key disappeared")
    };
    Ok(match previous {
        Some(value) => format!(
            "{}{}{}",
            &text[..start],
            rewrite(&text[start..end], value),
            &text[end..]
        ),
        None => format!("{}{}", &text[..start], &text[end..]),
    })
}
fn inserted(text: &str, value: &str) -> String {
    if text.ends_with('\n') {
        format!("{FILECHOOSER}={value}\n")
    } else {
        format!("\n{FILECHOOSER}={value}\n")
    }
}
fn rewrite(line: &str, value: &str) -> String {
    let (left, right) = line.split_once('=').expect("known assignment");
    let suffix = right
        .find('#')
        .map(|at| &right[at..])
        .unwrap_or_else(|| if line.ends_with('\n') { "\n" } else { "" });
    let spacing: String = right
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .collect();
    format!("{left}={spacing}{value}{suffix}")
}

fn state(path: &Path) -> Result<Option<State>> {
    match fs::read(path) {
        Ok(bytes) => {
            match serde_json::from_slice(&bytes).context("error: invalid portal routing state")? {
                StoredState::Current(state) => Ok(Some(state)),
                StoredState::Legacy(record) => Ok(Some(State {
                    records: vec![record],
                })),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredState {
    Current(State),
    Legacy(Record),
}
fn write_state(path: &Path, state: &State) -> Result<()> {
    atomic(path, &serde_json::to_vec(state)?, MODE)
}
fn remove_state(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn mode(path: &Path) -> Result<u32> {
    Ok(fs::metadata(path)
        .map(|meta| meta.mode() & 0o777)
        .unwrap_or(MODE))
}
fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}
fn is_missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
}

#[derive(Clone, Copy)]
enum Expected<'a> {
    Hash(&'a str),
    Missing,
}

fn recheck(path: &Path, expected: Expected<'_>) -> Result<()> {
    let matches = match expected {
        Expected::Hash(expected) => owned(path)
            .map(|(text, _)| hash(&text) == expected)
            .unwrap_or(false),
        Expected::Missing => fs::symlink_metadata(path).map(|_| false).or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(true)
            } else {
                Err(error)
            }
        })?,
    };
    if matches {
        Ok(())
    } else {
        anyhow::bail!("error: portal routing target changed; refusing to overwrite it")
    }
}

fn atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    atomic_inner(path, bytes, mode, None)
}

fn atomic_checked(path: &Path, bytes: &[u8], mode: u32, expected: Expected<'_>) -> Result<()> {
    atomic_inner(path, bytes, mode, Some(expected))
}

fn atomic_inner(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    expected: Option<Expected<'_>>,
) -> Result<()> {
    let parent = path.parent().context("error: portal path has no parent")?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .context("error: portal path has no file name")?
        .to_string_lossy();
    for attempt in 0..100 {
        let temporary = parent.join(format!(".{name}.elio-{}-{attempt}", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            if let Some(expected) = expected {
                recheck(path, expected)?;
            }
            fs::rename(&temporary, path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
            sync(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    anyhow::bail!("error: failed to create a temporary portal config")
}
fn sync(path: &Path) -> Result<()> {
    match File::open(path)?.sync_all() {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

struct Lock(File);
impl Lock {
    fn new(state: &Path) -> Result<Self> {
        let parent = state
            .parent()
            .context("error: routing state has no parent")?;
        fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(MODE)
            .open(parent.join("portal-routing.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn absolute(name: &str, fallback: Option<PathBuf>) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .or(fallback)
        .filter(|path| path.is_absolute())
        .with_context(|| format!("error: ${name} must be an absolute path"))
}
fn dirs(name: &str, fallback: &str) -> Result<Vec<PathBuf>> {
    env::var(name)
        .unwrap_or_else(|_| fallback.to_string())
        .split(':')
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                Ok(path)
            } else {
                anyhow::bail!("error: ${name} must contain absolute paths")
            }
        })
        .collect()
}
fn unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}
fn extend(paths: &mut Vec<PathBuf>, additions: Vec<PathBuf>) {
    for path in additions {
        unique(paths, path);
    }
}
