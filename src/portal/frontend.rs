//! Refreshes the xdg-desktop-portal frontend after routing changes.

use anyhow::{Context, Result};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use zbus::{Connection, fdo::DBusProxy};

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const SERVICE_FILE: &str = "org.freedesktop.portal.Desktop.service";
const SERVICE_RELATIVE_PATH: &str = "dbus-1/services";
const WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Eq, PartialEq)]
struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    source: Option<PathBuf>,
}

/// Replaces the session portal frontend and proves it acquired a new D-Bus owner.
pub(crate) fn reactivate() -> Result<()> {
    let command = resolve()?;
    zbus::block_on(async {
        let connection = Connection::session().await.context(
            "error: could not connect to the session D-Bus to refresh xdg-desktop-portal",
        )?;
        let bus = DBusProxy::new(&connection).await?;
        let before = owner(&bus).await?;

        let mut child = Command::new(&command.program);
        child
            .args(&command.args)
            .arg("--replace")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = child.spawn().with_context(|| match command.source {
            Some(ref source) => format!(
                "error: could not start xdg-desktop-portal from D-Bus service {}",
                source.display()
            ),
            None => "error: could not start xdg-desktop-portal from PATH fallback".to_string(),
        })?;

        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let after = owner(&bus).await?;
            if owner_changed(before.as_deref(), after.as_deref()) {
                return Ok(());
            }
            if let Some(status) = child
                .try_wait()
                .context("error: could not inspect xdg-desktop-portal after --replace")?
            {
                let source = command
                    .source
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "PATH fallback".to_string());
                anyhow::bail!(
                    "error: xdg-desktop-portal exited with {status} before acquiring a new D-Bus owner after --replace (resolved from {source})"
                )
            }
            if Instant::now() >= deadline {
                let source = command
                    .source
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "PATH fallback".to_string());
                anyhow::bail!(
                    "error: xdg-desktop-portal did not acquire a new D-Bus owner within {} seconds after --replace (resolved from {source})",
                    WAIT_TIMEOUT.as_secs()
                )
            }
            thread::sleep(POLL_INTERVAL);
        }
    })
}

fn owner_changed(before: Option<&str>, after: Option<&str>) -> bool {
    after.is_some_and(|after| before != Some(after))
}

async fn owner(bus: &DBusProxy<'_>) -> Result<Option<String>> {
    if !bus
        .name_has_owner(PORTAL_NAME.try_into().expect("valid D-Bus name"))
        .await
        .context("error: could not query xdg-desktop-portal on the session D-Bus")?
    {
        return Ok(None);
    }
    Ok(Some(
        bus.get_name_owner(PORTAL_NAME.try_into().expect("valid D-Bus name"))
            .await
            .context("error: could not read xdg-desktop-portal D-Bus owner")?
            .to_string(),
    ))
}

fn resolve() -> Result<CommandSpec> {
    let paths = service_paths()?;
    for directory in &paths {
        let file = directory.join(SERVICE_FILE);
        if !file.is_file() {
            continue;
        }
        return parse_service(&file)?.with_context(|| {
            format!(
                "error: D-Bus service {} does not define an Exec= for {PORTAL_NAME}",
                file.display()
            )
        });
    }
    Ok(CommandSpec {
        program: OsString::from("xdg-desktop-portal"),
        args: Vec::new(),
        source: None,
    })
}

fn service_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR").filter(|path| !path.is_empty()) {
        let runtime = absolute("XDG_RUNTIME_DIR", PathBuf::from(runtime))?;
        push_unique(&mut paths, runtime.join(SERVICE_RELATIVE_PATH));
    }
    let data_home = data_home(env::var_os("XDG_DATA_HOME"), || {
        Ok(home()?.join(".local/share"))
    })?;
    push_unique(&mut paths, data_home.join(SERVICE_RELATIVE_PATH));
    for data_dir in split_absolute_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share")? {
        push_unique(&mut paths, data_dir.join(SERVICE_RELATIVE_PATH));
    }
    // dbus-daemon also searches its build-time datadir, normally one of these.
    push_unique(
        &mut paths,
        PathBuf::from("/usr/local/share").join(SERVICE_RELATIVE_PATH),
    );
    push_unique(
        &mut paths,
        PathBuf::from("/usr/share").join(SERVICE_RELATIVE_PATH),
    );
    Ok(paths)
}

fn parse_service(path: &Path) -> Result<Option<CommandSpec>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("error: failed to read D-Bus service {}", path.display()))?;
    let mut in_service = false;
    let mut name = None;
    let mut exec = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_service = line == "[D-BUS Service]";
            continue;
        }
        if !in_service || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim()),
            "Exec" => exec = Some(value.trim()),
            _ => {}
        }
    }
    if name != Some(PORTAL_NAME) {
        anyhow::bail!(
            "error: D-Bus service {} does not declare Name={PORTAL_NAME}",
            path.display()
        )
    }
    let Some(exec) = exec else {
        return Ok(None);
    };
    let mut words = split_exec(exec)
        .with_context(|| format!("error: invalid Exec= in D-Bus service {}", path.display()))?;
    let program = words
        .first()
        .context("error: D-Bus service Exec= is empty")?
        .clone();
    if !Path::new(&program).is_absolute() {
        anyhow::bail!(
            "error: D-Bus service {} has a non-absolute Exec=; refusing to use PATH",
            path.display()
        )
    }
    Ok(Some(CommandSpec {
        program,
        args: words.drain(1..).collect(),
        source: Some(path.to_path_buf()),
    }))
}

fn split_exec(input: &str) -> Result<Vec<OsString>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            word.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                word.push(character);
            }
        } else if character.is_whitespace() && quote.is_none() {
            if !word.is_empty() {
                words.push(OsString::from(std::mem::take(&mut word)));
            }
        } else {
            word.push(character);
        }
    }
    if escaped || quote.is_some() {
        anyhow::bail!("unterminated escape or quote")
    }
    if !word.is_empty() {
        words.push(OsString::from(word));
    }
    Ok(words)
}

fn home() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("error: could not find an absolute $HOME for D-Bus service lookup")
}

fn data_home(
    value: Option<OsString>,
    default: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    match value.filter(|path| !path.is_empty()) {
        Some(path) => absolute("XDG_DATA_HOME", PathBuf::from(path)),
        None => default(),
    }
}

fn absolute(name: &str, path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        anyhow::bail!("error: ${name} must be an absolute path")
    }
}

fn split_absolute_dirs(name: &str, fallback: &str) -> Result<Vec<PathBuf>> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .map(|path| absolute(name, path))
        .collect()
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_replacement_owner_only_after_a_transition() {
        assert!(owner_changed(None, Some(":1.9")));
        assert!(owner_changed(Some(":1.4"), Some(":1.9")));
        assert!(!owner_changed(Some(":1.4"), Some(":1.4")));
        assert!(!owner_changed(None, None));
    }

    #[test]
    fn parses_the_standard_portal_activation_service() {
        let root = temporary_root("service");
        let service = root.join(SERVICE_FILE);
        fs::write(
            &service,
            "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=/usr/libexec/xdg-desktop-portal --verbose\n",
        )
        .unwrap();
        assert_eq!(
            parse_service(&service).unwrap(),
            Some(CommandSpec {
                program: OsString::from("/usr/libexec/xdg-desktop-portal"),
                args: vec![OsString::from("--verbose")],
                source: Some(service.clone()),
            })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_service_with_a_path_dependent_exec() {
        let root = temporary_root("relative-exec");
        let service = root.join(SERVICE_FILE);
        fs::write(
            &service,
            "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=xdg-desktop-portal\n",
        )
        .unwrap();
        assert!(
            parse_service(&service)
                .unwrap_err()
                .to_string()
                .contains("non-absolute")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_quoted_service_arguments_without_a_shell() {
        assert_eq!(
            split_exec("/usr/libexec/xdg-desktop-portal --flag='two words'").unwrap(),
            vec![
                OsString::from("/usr/libexec/xdg-desktop-portal"),
                OsString::from("--flag=two words"),
            ]
        );
    }

    #[test]
    fn empty_xdg_data_home_uses_its_default() {
        let default = PathBuf::from("/home/test/.local/share");
        assert_eq!(
            data_home(Some(OsString::new()), || Ok(default.clone())).unwrap(),
            default
        );
    }

    fn temporary_root(label: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "elio-portal-frontend-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
