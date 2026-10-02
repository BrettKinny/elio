#![cfg(unix)]

#[allow(dead_code)]
mod support;

use std::{
    error::Error,
    ffi::OsString,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use support::temp_path;

const FAKE_ELIO: &str = r#"#!/bin/sh
if [ "$1" = "--cwd-file" ]; then
  if [ -n "${ELIO_TEST_TEMP_LOG-}" ]; then
    printf '%s' "$2" > "$ELIO_TEST_TEMP_LOG"
  fi
  if [ "${3-}" = "empty" ]; then
    : > "$2"
    exit 9
  fi
  printf '%s' "$ELIO_TEST_DESTINATION" > "$2"
  exit 7
fi

if [ "$1" = "--version" ] || [ "$1" = "-V" ]; then
  printf 'VERSION-PASSTHROUGH\n'
  exit 0
fi

if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  printf 'HELP-PASSTHROUGH\n'
  exit 3
fi

if [ "$1" = "shell" ]; then
  printf 'SHELL-PASSTHROUGH\n'
  exit 4
fi

exit 5
"#;

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let path = temp_path(label);
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct RuntimeFixture {
    _root: TempRoot,
    fake_dir: PathBuf,
    init_script: PathBuf,
    start_dir: PathBuf,
    destination: PathBuf,
    home_dir: PathBuf,
    config_home: PathBuf,
    data_home: PathBuf,
    zdotdir: PathBuf,
}

#[derive(Clone, Copy)]
enum ShellSyntax {
    Posix,
    Fish,
    Nu,
    Pwsh,
}

#[test]
fn generated_bash_function_runs_when_executed() -> Result<(), Box<dyn Error>> {
    run_generated_function("bash", ShellSyntax::Posix)
}

#[test]
fn generated_zsh_function_runs_when_executed() -> Result<(), Box<dyn Error>> {
    run_generated_function("zsh", ShellSyntax::Posix)
}

#[test]
fn generated_fish_function_runs_when_executed() -> Result<(), Box<dyn Error>> {
    run_generated_function("fish", ShellSyntax::Fish)
}

#[test]
fn generated_nu_function_runs_when_executed() -> Result<(), Box<dyn Error>> {
    run_generated_function("nu", ShellSyntax::Nu)
}

#[test]
fn generated_pwsh_function_runs_when_executed() -> Result<(), Box<dyn Error>> {
    run_generated_function("pwsh", ShellSyntax::Pwsh)
}

#[test]
fn generated_nu_function_follows_directory_symlinks() -> Result<(), Box<dyn Error>> {
    run_generated_function_with_destination("nu", ShellSyntax::Nu, true)
}

fn run_generated_function(shell: &str, syntax: ShellSyntax) -> Result<(), Box<dyn Error>> {
    run_generated_function_with_destination(shell, syntax, false)
}

fn run_generated_function_with_destination(
    shell: &str,
    syntax: ShellSyntax,
    use_symlink: bool,
) -> Result<(), Box<dyn Error>> {
    if !shell_available(shell) {
        return Ok(());
    }

    let fixture = runtime_fixture(shell)?;
    let runtime_script = match syntax {
        ShellSyntax::Posix => posix_runtime_script(&fixture.init_script, &fixture.start_dir),
        ShellSyntax::Fish => fish_runtime_script(&fixture.init_script, &fixture.start_dir),
        ShellSyntax::Nu => nu_runtime_script(&fixture.init_script, &fixture.start_dir),
        ShellSyntax::Pwsh => pwsh_runtime_script(&fixture.init_script, &fixture.start_dir),
    };

    let mut command = Command::new(shell);
    if matches!(syntax, ShellSyntax::Fish) {
        command.arg("--no-config");
    }
    if matches!(syntax, ShellSyntax::Nu) {
        command.arg("--no-config-file");
    }
    if matches!(syntax, ShellSyntax::Pwsh) {
        command.args(["-NoLogo", "-NoProfile"]);
    }
    command.arg("-c").arg(runtime_script);
    configure_runtime_environment(&mut command, &fixture)?;
    let destination = if use_symlink {
        let link = fixture._root.path().join("destination link");
        symlink(&fixture.destination, &link)?;
        command.env("ELIO_TEST_DESTINATION", &link);
        link
    } else {
        fixture.destination.clone()
    };
    let output = command.output()?;

    assert_runtime_output(output, &fixture.start_dir, &destination);
    Ok(())
}

fn runtime_fixture(shell: &str) -> Result<RuntimeFixture, Box<dyn Error>> {
    let root = TempRoot::new(&format!("shell-runtime-{shell}"))?;
    let gen_dir = root.path().join("gen-bin");
    let fake_dir = root.path().join("fake-bin");
    let start_dir = root.path().join("start");
    let destination = root.path().join("destination with spaces");
    fs::create_dir_all(&destination)?;
    let destination = destination.canonicalize()?;
    let home_dir = root.path().join("home");
    let config_home = root.path().join("config");
    let data_home = root.path().join("data");
    let zdotdir = root.path().join("zdotdir");
    fs::create_dir_all(&gen_dir)?;
    fs::create_dir_all(&fake_dir)?;
    fs::create_dir_all(&start_dir)?;
    fs::create_dir_all(&home_dir)?;
    fs::create_dir_all(&config_home)?;
    fs::create_dir_all(&data_home)?;
    fs::create_dir_all(&zdotdir)?;

    symlink(env!("CARGO_BIN_EXE_elio"), gen_dir.join("elio"))?;
    write_fake_elio(&fake_dir.join("elio"))?;

    let output = Command::new("elio")
        .args(["shell", "init", shell])
        .env("PATH", path_with_prefix(&gen_dir)?)
        .output()?;
    assert!(
        output.status.success(),
        "failed to generate {shell} init script\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr while generating {shell} init script:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let script = String::from_utf8(output.stdout)?;
    let expected_path_call = if shell == "nu" {
        r#"run-external "elio""#
    } else if shell == "pwsh" {
        "-CommandType Application"
    } else {
        "command elio"
    };
    assert!(
        script.contains(expected_path_call),
        "official-install {shell} init script should call elio from PATH:\n{script}"
    );
    assert!(
        !script.contains(env!("CARGO_BIN_EXE_elio")),
        "official-install {shell} init script should not contain the test binary path:\n{script}"
    );
    assert!(
        !script.contains("target/debug/elio"),
        "official-install {shell} init script should not contain target/debug/elio:\n{script}"
    );

    let extension = if shell == "pwsh" { "ps1" } else { shell };
    let init_script = root.path().join(format!("init.{extension}"));
    fs::write(&init_script, script)?;

    Ok(RuntimeFixture {
        _root: root,
        fake_dir,
        init_script,
        start_dir,
        destination,
        home_dir,
        config_home,
        data_home,
        zdotdir,
    })
}

fn configure_runtime_environment(
    command: &mut Command,
    fixture: &RuntimeFixture,
) -> Result<(), Box<dyn Error>> {
    command
        .env("PATH", path_with_prefix(&fixture.fake_dir)?)
        .env("ELIO_TEST_DESTINATION", &fixture.destination)
        .env("HOME", &fixture.home_dir)
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env("ZDOTDIR", &fixture.zdotdir)
        .env_remove("BASH_ENV")
        .env_remove("ENV");

    Ok(())
}

fn write_fake_elio(path: &Path) -> Result<(), Box<dyn Error>> {
    fs::write(path, FAKE_ELIO)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn posix_runtime_script(init_script: &Path, start_dir: &Path) -> String {
    format!(
        r#"source {}
mkdir -p {}
cd {}

elio
printf 'cwd=%s code=%s\n' "$PWD" "$?"

cd {}
elio empty
printf 'empty_cwd=%s empty_code=%s\n' "$PWD" "$?"

cd {}
elio --chooser-file /tmp/elio-choice child
printf 'chooser_flag_first_cwd=%s chooser_flag_first_code=%s\n' "$PWD" "$?"

cd {}
elio child --chooser-file /tmp/elio-choice
printf 'chooser_cwd=%s chooser_code=%s\n' "$PWD" "$?"

cd {}
elio child --chooser-file=/tmp/elio-choice
printf 'chooser_equals_cwd=%s chooser_equals_code=%s\n' "$PWD" "$?"

elio --help
printf 'help_code=%s\n' "$?"

elio shell status
printf 'shell_code=%s\n' "$?"
"#,
        shell_quote(init_script),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
    )
}

fn fish_runtime_script(init_script: &Path, start_dir: &Path) -> String {
    format!(
        r#"source {}
mkdir -p {}
cd {}

elio
printf 'cwd=%s code=%s\n' "$PWD" "$status"

cd {}
elio empty
printf 'empty_cwd=%s empty_code=%s\n' "$PWD" "$status"

cd {}
elio --chooser-file /tmp/elio-choice child
printf 'chooser_flag_first_cwd=%s chooser_flag_first_code=%s\n' "$PWD" "$status"

cd {}
elio child --chooser-file /tmp/elio-choice
printf 'chooser_cwd=%s chooser_code=%s\n' "$PWD" "$status"

cd {}
elio child --chooser-file=/tmp/elio-choice
printf 'chooser_equals_cwd=%s chooser_equals_code=%s\n' "$PWD" "$status"

elio --help
printf 'help_code=%s\n' "$status"

elio shell status
printf 'shell_code=%s\n' "$status"
"#,
        shell_quote(init_script),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
        shell_quote(start_dir),
    )
}

fn nu_runtime_script(init_script: &Path, start_dir: &Path) -> String {
    let pipeline_output = start_dir.join("nu-shell-pipeline.txt");
    format!(
        r#"source {}
mkdir {}
cd {}

elio
print $"cwd=($env.PWD) code=($env.LAST_EXIT_CODE)"

cd {}
elio empty
print $"empty_cwd=($env.PWD) empty_code=($env.LAST_EXIT_CODE)"

cd {}
elio --chooser-file /tmp/elio-choice child
print $"chooser_flag_first_cwd=($env.PWD) chooser_flag_first_code=($env.LAST_EXIT_CODE)"

cd {}
elio child --chooser-file /tmp/elio-choice
print $"chooser_cwd=($env.PWD) chooser_code=($env.LAST_EXIT_CODE)"

cd {}
elio child --chooser-file=/tmp/elio-choice
print $"chooser_equals_cwd=($env.PWD) chooser_equals_code=($env.LAST_EXIT_CODE)"

elio --help
print $"help_code=($env.LAST_EXIT_CODE)"

elio shell status
print $"shell_code=($env.LAST_EXIT_CODE)"

elio shell status | save -f {}
print $"shell_pipeline_code=($env.LAST_EXIT_CODE)"
print $"shell_pipeline=(open --raw {})"
"#,
        nu_quote(init_script),
        nu_quote(start_dir),
        nu_quote(start_dir),
        nu_quote(start_dir),
        nu_quote(start_dir),
        nu_quote(start_dir),
        nu_quote(start_dir),
        nu_quote(&pipeline_output),
        nu_quote(&pipeline_output),
    )
}

fn pwsh_runtime_script(init_script: &Path, start_dir: &Path) -> String {
    format!(
        r#". {init}
$ErrorActionPreference = 'Continue'
Set-Location -LiteralPath {start}
elio 2>$null
"cwd=$PWD code=$LASTEXITCODE"

Set-Location -LiteralPath {start}
elio empty 2>$null
"empty_cwd=$PWD empty_code=$LASTEXITCODE"

Set-Location -LiteralPath {start}
elio --chooser-file /tmp/elio-choice child 2>$null
"chooser_flag_first_cwd=$PWD chooser_flag_first_code=$LASTEXITCODE"
elio child --chooser-file /tmp/elio-choice 2>$null
"chooser_cwd=$PWD chooser_code=$LASTEXITCODE"
elio child --chooser-file=/tmp/elio-choice 2>$null
"chooser_equals_cwd=$PWD chooser_equals_code=$LASTEXITCODE"
elio --help 2>$null
"help_code=$LASTEXITCODE"
elio shell status 2>$null
"shell_code=$LASTEXITCODE"

foreach ($callArgs in @(@('empty'), @('--help'), @('-h'), @('shell', 'status'), @('child', '--chooser-file', '/tmp/elio-choice'))) {{
    elio @callArgs 2>$null
    if ($?) {{ throw 'Failed elio call reported success' }}
    elio @callArgs 2>$null && $(throw 'Failed elio call entered && branch')
    $recovered = $false
    elio @callArgs 2>$null || $($recovered = $true)
    if (-not $recovered) {{ throw 'Failed elio call skipped || branch' }}
}}

foreach ($versionFlag in @('--version', '-V')) {{
    $version = elio $versionFlag
    if (-not $? -or $LASTEXITCODE -ne 0 -or $version -ne 'VERSION-PASSTHROUGH') {{
        throw 'Successful version call was not forwarded correctly'
    }}
    elio $versionFlag && 'success branch' || $(throw 'Successful elio call entered || branch')
}}

# Literal -V goes through PowerShell parameter binding; an array splat does not.
$version = elio -V
if (-not $? -or $LASTEXITCODE -ne 0 -or $version -ne 'VERSION-PASSTHROUGH') {{
    throw 'Literal -V was consumed as a PowerShell common parameter'
}}

$env:ELIO_TEST_TEMP_LOG = {start} + '/temp-path'
$PSNativeCommandUseErrorActionPreference = $true
$ErrorActionPreference = 'Stop'
$caught = $false
try {{ elio empty }} catch {{ $caught = $true }}
if (-not $caught -or $LASTEXITCODE -ne 9) {{ throw 'Terminating error lost the exit code' }}
$tmp = Get-Content -LiteralPath $env:ELIO_TEST_TEMP_LOG -Raw
if (Test-Path -LiteralPath $tmp) {{ throw 'Terminating error leaked the temporary file' }}
$ErrorActionPreference = 'Continue'

# On case-sensitive filesystems, start and START are different directories.
$caseDestination = {start} + '/START'
$caseStart = {start} + '/start'
New-Item -ItemType Directory -Force $caseStart, $caseDestination >$null
if (-not (Test-Path -LiteralPath ($caseStart + '/probe'))) {{
    Set-Content -LiteralPath ($caseDestination + '/probe') -Value 'case probe'
    if (-not (Test-Path -LiteralPath ($caseStart + '/probe'))) {{
        Set-Location -LiteralPath $caseStart
        $env:ELIO_TEST_DESTINATION = $caseDestination
        elio 2>$null
        if ($PWD.Path -cne $caseDestination) {{ throw 'Case-only directory change was skipped' }}
    }}
}}
exit 0
"#,
        init = pwsh_quote(init_script),
        start = pwsh_quote(start_dir),
    )
}

fn pwsh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn assert_runtime_output(output: std::process::Output, start_dir: &Path, destination: &Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "shell runtime script failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.is_empty(),
        "shell runtime script printed stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("cwd={} code=7", destination.display())),
        "normal call should cd to the destination and return fake status\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("empty_cwd={} empty_code=9", start_dir.display())),
        "empty cwd file should leave the shell in the original directory\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "chooser_flag_first_cwd={} chooser_flag_first_code=5",
            start_dir.display()
        )),
        "flag-first chooser calls should pass through without cwd integration\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "chooser_cwd={} chooser_code=5",
            start_dir.display()
        )),
        "path-first chooser calls should pass through without cwd integration\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "chooser_equals_cwd={} chooser_equals_code=5",
            start_dir.display()
        )),
        "path-first chooser= calls should pass through without cwd integration\nstdout:\n{stdout}"
    );
    let has_nu_pipeline_checks = stdout.contains("shell_pipeline_code=");
    if !has_nu_pipeline_checks {
        assert!(
            stdout.contains("HELP-PASSTHROUGH"),
            "--help should pass through to the real executable\nstdout:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("help_code=3"),
        "--help should preserve the real executable status\nstdout:\n{stdout}"
    );
    if !has_nu_pipeline_checks {
        assert!(
            stdout.contains("SHELL-PASSTHROUGH"),
            "shell subcommands should pass through to the real executable\nstdout:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("shell_code=4"),
        "shell subcommands should preserve the real executable status\nstdout:\n{stdout}"
    );
    if has_nu_pipeline_checks {
        assert!(
            stdout.contains("shell_pipeline_code=4"),
            "Nu shell subcommand pipelines should preserve the real executable status\nstdout:\n{stdout}"
        );
        assert!(
            stdout.contains("shell_pipeline=SHELL-PASSTHROUGH"),
            "Nu shell subcommand pipelines should receive the real executable stdout\nstdout:\n{stdout}"
        );
    }
}

fn shell_available(shell: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {shell}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn path_with_prefix(prefix: &Path) -> Result<OsString, Box<dyn Error>> {
    let mut paths = vec![prefix.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    Ok(std::env::join_paths(paths)?)
}

fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn nu_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
