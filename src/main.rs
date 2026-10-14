#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

#[cfg(not(windows))]
compile_error!("drvload only supports Windows");

#[cfg(windows)]
mod windows;

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

const USAGE: &str = r#"drvload - dynamically load a Windows kernel driver or minifilter

USAGE:
    drvload [OPTIONS] <DRIVER.sys>

OPTIONS:
    -t, --type <driver|minifilter>  Driver kind (default: driver, or minifilter
                                     when --altitude is present)
    -n, --name <SERVICE_NAME>       Service name (default: drvload_<file-stem>)
    -a, --altitude <ALTITUDE>       Minifilter altitude; implies minifilter
    -i, --instance <INSTANCE_NAME>  Minifilter instance name
    -g, --load-order-group <GROUP>  Minifilter load-order group
                                     (default: FSFilter Activity Monitor)
        --no-elevate                Fail instead of displaying a UAC prompt
    -h, --help                      Print help
    -V, --version                   Print version

EXAMPLES:
    drvload .\mydriver.sys
    drvload --type minifilter --altitude 370030 .\myfilter.sys

The driver must be built for the running architecture and accepted by Windows'
driver-signing policy. Minifilter altitudes used in production must be allocated
by Microsoft; fractional values derived from an allocated altitude are permitted.
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriverKind {
    Kernel,
    Minifilter,
}

#[derive(Debug, Clone)]
struct Options {
    path: PathBuf,
    kind: DriverKind,
    service_name: String,
    altitude: Option<String>,
    instance_name: Option<String>,
    load_order_group: String,
    no_elevate: bool,
    elevated_child: bool,
}

#[derive(Debug)]
struct AppError {
    message: String,
    native_code: Option<u32>,
}

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            native_code: None,
        }
    }

    fn native(message: impl Into<String>, code: u32) -> Self {
        Self {
            message: message.into(),
            native_code: Some(code),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(code) = self.native_code {
            #[cfg(windows)]
            write!(
                f,
                "\n\nWindows error {code} (0x{code:08X}): {}",
                windows::format_error(code)
            )?;
            #[cfg(not(windows))]
            write!(f, "\n\nOS error {code} (0x{code:08X})")?;
            if let Some(hint) = native_error_hint(code) {
                write!(f, "\n\nHint: {hint}")?;
            }
        }
        Ok(())
    }
}

fn native_error_hint(code: u32) -> Option<&'static str> {
    match code {
        2 | 3 => Some(
            "Verify that the .sys path is local, absolute or resolvable, and still exists after elevation.",
        ),
        5 | 1314 => Some(
            "Administrator rights and SeLoadDriverPrivilege are required. Check UAC and local security policy.",
        ),
        193 | 216 => Some(
            "The driver may target the wrong CPU architecture or may not be a valid Windows driver image.",
        ),
        577 => Some(
            "Windows rejected the image signature. Use a properly signed driver, or an appropriate test-signing environment for development.",
        ),
        1058 => Some(
            "The driver service is disabled. Use a different service name or correct its existing service configuration.",
        ),
        1067 => Some(
            "The driver's DriverEntry routine failed. Check the System event log and kernel debugger output for the driver's status code.",
        ),
        1072 => Some(
            "The service is marked for deletion. Close programs holding service handles, wait briefly, or reboot before retrying.",
        ),
        1275 => Some(
            "Windows policy blocked this driver. Check Code Integrity, vulnerable-driver blocklist, HVCI/Memory Integrity, and event logs.",
        ),
        0x801F_0011 => Some(
            "A minifilter instance already uses this altitude on a volume. Use the altitude allocated for this filter.",
        ),
        0x801F_0012 => Some(
            "A minifilter instance with this name already exists. Choose a unique value with --instance.",
        ),
        _ => None,
    }
}

enum ParseResult {
    Run(Options),
    ExitSuccess(String),
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<ParseResult, AppError> {
    let mut it = args.into_iter();
    let _program = it.next();
    let mut path = None;
    let mut kind = None;
    let mut service_name = None;
    let mut altitude = None;
    let mut instance_name = None;
    let mut load_order_group = "FSFilter Activity Monitor".to_owned();
    let mut no_elevate = false;
    let mut elevated_child = false;

    let mut rest = it.peekable();
    while let Some(arg) = rest.next() {
        let mut value_for = |flag: &str| {
            rest.next().ok_or_else(|| {
                AppError::new(format!("option '{flag}' requires a value\n\n{USAGE}"))
            })
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParseResult::ExitSuccess(USAGE.to_owned())),
            "-V" | "--version" => {
                return Ok(ParseResult::ExitSuccess(format!(
                    "drvload {}\n",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            "-t" | "--type" => {
                let value = value_for(&arg)?;
                kind = Some(match value.as_str() {
                    "driver" | "kernel" => DriverKind::Kernel,
                    "minifilter" | "filter" => DriverKind::Minifilter,
                    _ => {
                        return Err(AppError::new(format!(
                            "invalid driver type '{value}'; expected 'driver' or 'minifilter'"
                        )));
                    }
                });
            }
            "-n" | "--name" => service_name = Some(value_for(&arg)?),
            "-a" | "--altitude" => altitude = Some(value_for(&arg)?),
            "-i" | "--instance" => instance_name = Some(value_for(&arg)?),
            "-g" | "--load-order-group" => load_order_group = value_for(&arg)?,
            "--no-elevate" => no_elevate = true,
            "--__elevated-child" => elevated_child = true,
            "--" => {
                for positional in rest.by_ref() {
                    if path.replace(PathBuf::from(&positional)).is_some() {
                        return Err(AppError::new("more than one driver path was provided"));
                    }
                }
            }
            _ if arg.starts_with('-') => {
                return Err(AppError::new(format!("unknown option '{arg}'\n\n{USAGE}")));
            }
            _ => {
                if path.replace(PathBuf::from(&arg)).is_some() {
                    return Err(AppError::new("more than one driver path was provided"));
                }
            }
        }
    }

    let path = path.ok_or_else(|| AppError::new(format!("missing driver path\n\n{USAGE}")))?;
    let kind = kind.unwrap_or(if altitude.is_some() {
        DriverKind::Minifilter
    } else {
        DriverKind::Kernel
    });
    if kind == DriverKind::Minifilter && altitude.is_none() {
        return Err(AppError::new(
            "--altitude is required when --type minifilter is used",
        ));
    }
    if kind == DriverKind::Kernel && (altitude.is_some() || instance_name.is_some()) {
        return Err(AppError::new(
            "--altitude and --instance are only valid for minifilters",
        ));
    }
    if let Some(value) = altitude.as_deref() {
        validate_altitude(value)?;
    }

    let service_name = service_name.unwrap_or_else(|| default_service_name(&path));
    validate_name("service", &service_name)?;
    let instance_name = match instance_name {
        Some(value) => {
            validate_name("instance", &value)?;
            Some(value)
        }
        None if kind == DriverKind::Minifilter => Some(format!("{service_name} Default Instance")),
        None => None,
    };
    if load_order_group.is_empty() || load_order_group.contains('\0') {
        return Err(AppError::new(
            "load-order group must not be empty or contain a NUL character",
        ));
    }

    Ok(ParseResult::Run(Options {
        path,
        kind,
        service_name,
        altitude,
        instance_name,
        load_order_group,
        no_elevate,
        elevated_child,
    }))
}

fn validate_altitude(value: &str) -> Result<(), AppError> {
    let mut dots = 0;
    if value.is_empty() || value.starts_with('.') || value.ends_with('.') {
        return Err(AppError::new(format!(
            "invalid minifilter altitude '{value}': expected decimal digits with at most one internal decimal point"
        )));
    }
    for ch in value.chars() {
        if ch == '.' {
            dots += 1;
        } else if !ch.is_ascii_digit() {
            return Err(AppError::new(format!(
                "invalid minifilter altitude '{value}': '{ch}' is not a decimal digit"
            )));
        }
    }
    if dots > 1 {
        return Err(AppError::new(format!(
            "invalid minifilter altitude '{value}': more than one decimal point"
        )));
    }
    Ok(())
}

fn validate_name(kind: &str, value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.encode_utf16().count() > 256 {
        return Err(AppError::new(format!(
            "{kind} name must contain 1 to 256 UTF-16 code units"
        )));
    }
    if value
        .chars()
        .any(|c| c == '\0' || c == '/' || c == '\\' || c.is_control())
    {
        return Err(AppError::new(format!(
            "{kind} name contains a forbidden slash, backslash, NUL, or control character"
        )));
    }
    Ok(())
}

fn default_service_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("driver");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(220)
        .collect();
    format!("drvload_{}", if safe.is_empty() { "driver" } else { &safe })
}

#[cfg(windows)]
fn run(options: &Options) -> Result<(), AppError> {
    let canonical = options.path.canonicalize().map_err(|e| {
        AppError::new(format!(
            "cannot resolve driver path '{}': {e}",
            options.path.display()
        ))
    })?;
    let metadata = canonical
        .metadata()
        .map_err(|e| AppError::new(format!("cannot inspect '{}': {e}", canonical.display())))?;
    if !metadata.is_file() {
        return Err(AppError::new(format!(
            "driver path is not a regular file: '{}'",
            canonical.display()
        )));
    }
    if canonical
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.eq_ignore_ascii_case("sys"))
        .unwrap_or(true)
    {
        return Err(AppError::new(format!(
            "driver path must name a .sys file: '{}'",
            canonical.display()
        )));
    }

    if !windows::is_elevated()? {
        if options.no_elevate || options.elevated_child {
            return Err(AppError::new(
                "administrator privileges are required, but this process is not elevated",
            ));
        }
        let exit_code = windows::relaunch_elevated()?;
        if exit_code == 0 {
            return Ok(());
        }
        return Err(AppError::new(format!(
            "the elevated drvload process failed with exit code {exit_code}"
        )));
    }

    windows::load(options, &canonical)
}

fn main() {
    let result = match parse_args(env::args()) {
        Ok(ParseResult::ExitSuccess(text)) => {
            print!("{text}");
            return;
        }
        Ok(ParseResult::Run(options)) => {
            #[cfg(windows)]
            {
                run(&options)
            }
            #[cfg(not(windows))]
            {
                let _ = options;
                Err(AppError::new("drvload only supports Windows"))
            }
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("drvload: error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altitude_validation() {
        for valid in ["0", "370000", "100.123456", "03333"] {
            assert!(validate_altitude(valid).is_ok(), "{valid}");
        }
        for invalid in ["", ".1", "1.", "1.2.3", "-1", "1e4", "abc"] {
            assert!(validate_altitude(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn altitude_implies_minifilter() {
        let args = ["drvload", "--altitude", "370000", "x.sys"].map(str::to_owned);
        let ParseResult::Run(options) = parse_args(args).unwrap() else {
            panic!()
        };
        assert_eq!(options.kind, DriverKind::Minifilter);
        assert_eq!(options.service_name, "drvload_x");
    }

    #[test]
    fn default_name_is_safe() {
        assert_eq!(
            default_service_name(Path::new("C:/a b/x y!.sys")),
            "drvload_x_y_"
        );
    }
}
