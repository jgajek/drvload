use crate::{AppError, DriverKind, Options};
use std::ffi::{OsStr, c_void};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

type Bool = i32;
type Handle = *mut c_void;
type ScHandle = *mut c_void;
type HKey = *mut c_void;

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
const ERROR_SERVICE_EXISTS: u32 = 1073;
const ERROR_CANCELLED: u32 = 1223;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x0000_1000;
const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x0000_0200;
const SC_MANAGER_CONNECT: u32 = 0x0001;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
const SERVICE_QUERY_CONFIG: u32 = 0x0001;
const SERVICE_START: u32 = 0x0010;
const DELETE: u32 = 0x0001_0000;
const SERVICE_KERNEL_DRIVER: u32 = 0x0000_0001;
const SERVICE_FILE_SYSTEM_DRIVER: u32 = 0x0000_0002;
const SERVICE_DEMAND_START: u32 = 0x0000_0003;
const SERVICE_ERROR_NORMAL: u32 = 0x0000_0001;
const REG_OPTION_NON_VOLATILE: u32 = 0;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_CREATE_SUB_KEY: u32 = 0x0004;
const KEY_WOW64_64KEY: u32 = 0x0100;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SW_SHOWNORMAL: i32 = 1;
const INFINITE: u32 = 0xffff_ffff;
const HKEY_LOCAL_MACHINE: HKey = 0x8000_0002usize as HKey;

#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: Handle,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: Handle,
    lp_id_list: *mut c_void,
    lp_class: *const u16,
    hkey_class: HKey,
    dw_hot_key: u32,
    h_icon_or_monitor: Handle,
    h_process: Handle,
}

#[repr(C)]
struct QueryServiceConfigW {
    service_type: u32,
    start_type: u32,
    error_control: u32,
    binary_path_name: *mut u16,
    load_order_group: *mut u16,
    tag_id: u32,
    dependencies: *mut u16,
    service_start_name: *mut u16,
    display_name: *mut u16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        message_id: u32,
        language_id: u32,
        buffer: *mut u16,
        size: u32,
        arguments: *const c_void,
    ) -> u32;
    fn GetModuleFileNameW(module: Handle, filename: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> Bool;
    fn CloseHandle(object: Handle) -> Bool;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn IsUserAnAdmin() -> Bool;
    fn ShellExecuteExW(exec_info: *mut ShellExecuteInfoW) -> Bool;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenSCManagerW(
        machine_name: *const u16,
        database_name: *const u16,
        desired_access: u32,
    ) -> ScHandle;
    fn CreateServiceW(
        manager: ScHandle,
        service_name: *const u16,
        display_name: *const u16,
        desired_access: u32,
        service_type: u32,
        start_type: u32,
        error_control: u32,
        binary_path: *const u16,
        load_order_group: *const u16,
        tag_id: *mut u32,
        dependencies: *const u16,
        service_start_name: *const u16,
        password: *const u16,
    ) -> ScHandle;
    fn OpenServiceW(manager: ScHandle, service_name: *const u16, desired_access: u32) -> ScHandle;
    fn QueryServiceConfigW(
        service: ScHandle,
        config: *mut QueryServiceConfigW,
        buffer_size: u32,
        bytes_needed: *mut u32,
    ) -> Bool;
    fn StartServiceW(service: ScHandle, argc: u32, argv: *const *const u16) -> Bool;
    fn DeleteService(service: ScHandle) -> Bool;
    fn CloseServiceHandle(handle: ScHandle) -> Bool;
    fn RegCreateKeyExW(
        key: HKey,
        subkey: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        sam: u32,
        security: *const c_void,
        result: *mut HKey,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: HKey,
        value_name: *const u16,
        reserved: u32,
        value_type: u32,
        data: *const u8,
        data_size: u32,
    ) -> i32;
    fn RegCloseKey(key: HKey) -> i32;
}

#[link(name = "FltLib")]
unsafe extern "system" {
    fn FilterLoad(filter_name: *const u16) -> i32;
}

struct ServiceHandle(ScHandle);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

struct RegistryKey(HKey);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

unsafe fn wide_ptr_to_string(mut ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let start = ptr;
    while unsafe { *ptr } != 0 {
        ptr = unsafe { ptr.add(1) };
    }
    String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(start, ptr.offset_from(start) as usize)
    })
}

pub fn format_error(code: u32) -> String {
    let mut buffer = [0u16; 2048];
    let length = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            null(),
            code,
            0,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            null(),
        )
    };
    if length == 0 {
        return "No system-provided description is available.".to_owned();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
        .trim()
        .to_owned()
}

pub fn is_elevated() -> Result<bool, AppError> {
    Ok(unsafe { IsUserAnAdmin() } != 0)
}

fn quote_windows_arg(arg: &OsStr) -> Vec<u16> {
    let value: Vec<u16> = arg.encode_wide().collect();
    if !value.is_empty()
        && !value
            .iter()
            .any(|c| matches!(*c, 0x09..=0x0d | 0x20 | 0x22))
    {
        return value;
    }
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for ch in value {
        if ch == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if ch == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
            result.push(b'"' as u16);
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
            result.push(ch);
        }
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    result
}

fn service_binary_path(path: &Path) -> Result<Vec<u16>, AppError> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    const EXTENDED_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const UNC_PREFIX: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    if value.starts_with(UNC_PREFIX)
        || (value.starts_with(&[b'\\' as u16, b'\\' as u16]) && !value.starts_with(EXTENDED_PREFIX))
    {
        return Err(AppError::new(format!(
            "driver image must be on a local volume; network path '{}' cannot be loaded by the kernel",
            path.display()
        )));
    }
    if value.starts_with(EXTENDED_PREFIX) {
        value.drain(..EXTENDED_PREFIX.len());
    }
    if value.contains(&0) {
        return Err(AppError::new("driver path contains a NUL character"));
    }
    value.push(0);
    Ok(value)
}

pub fn relaunch_elevated() -> Result<u32, AppError> {
    let mut executable = vec![0u16; 32768];
    let length =
        unsafe { GetModuleFileNameW(null_mut(), executable.as_mut_ptr(), executable.len() as u32) };
    if length == 0 || length as usize >= executable.len() {
        return Err(AppError::native(
            "cannot determine the drvload executable path for elevation",
            unsafe { GetLastError() },
        ));
    }
    executable.truncate(length as usize + 1);

    // Put the internal marker before the original arguments so it remains an
    // option even if the caller used `--` before the driver path.
    let mut parameters = quote_windows_arg(OsStr::new("--__elevated-child"));
    for arg in std::env::args_os().skip(1) {
        parameters.push(b' ' as u16);
        parameters.extend(quote_windows_arg(&arg));
    }
    parameters.push(0);
    let verb = wide("runas");
    let current_directory = std::env::current_dir().map_err(|error| {
        AppError::new(format!(
            "cannot determine the current directory for the elevated process: {error}"
        ))
    })?;
    let current_directory = wide(current_directory.as_os_str());
    let mut info: ShellExecuteInfoW = unsafe { zeroed() };
    info.cb_size = size_of::<ShellExecuteInfoW>() as u32;
    info.f_mask = SEE_MASK_NOCLOSEPROCESS;
    info.lp_verb = verb.as_ptr();
    info.lp_file = executable.as_ptr();
    info.lp_parameters = parameters.as_ptr();
    info.lp_directory = current_directory.as_ptr();
    info.n_show = SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code = unsafe { GetLastError() };
        let message = if code == ERROR_CANCELLED {
            "UAC elevation was cancelled"
        } else {
            "cannot launch the elevated drvload process"
        };
        return Err(AppError::native(message, code));
    }
    if info.h_process.is_null() {
        return Err(AppError::new(
            "elevation succeeded but Windows did not return a process handle",
        ));
    }
    unsafe {
        WaitForSingleObject(info.h_process, INFINITE);
    }
    let mut exit_code = 1;
    let ok = unsafe { GetExitCodeProcess(info.h_process, &mut exit_code) };
    unsafe {
        CloseHandle(info.h_process);
    }
    if ok == 0 {
        return Err(AppError::native(
            "cannot obtain the elevated process exit code",
            unsafe { GetLastError() },
        ));
    }
    Ok(exit_code)
}

pub fn load(options: &Options, canonical_path: &Path) -> Result<(), AppError> {
    let manager = unsafe {
        OpenSCManagerW(
            null(),
            null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(AppError::native(
            "cannot open the Service Control Manager",
            unsafe { GetLastError() },
        ));
    }
    let manager = ServiceHandle(manager);

    let service_type = match options.kind {
        DriverKind::Kernel => SERVICE_KERNEL_DRIVER,
        DriverKind::Minifilter => SERVICE_FILE_SYSTEM_DRIVER,
    };
    let service_name = wide(&options.service_name);
    let display_name = wide(format!("drvload: {}", options.service_name));
    let binary_path = service_binary_path(canonical_path)?;
    let normalized_path = String::from_utf16_lossy(&binary_path[..binary_path.len() - 1]);
    let group = wide(&options.load_order_group);
    let dependencies = wide("FltMgr\0"); // wide() adds the second terminating NUL.
    let (group_ptr, dependency_ptr) = if options.kind == DriverKind::Minifilter {
        (group.as_ptr(), dependencies.as_ptr())
    } else {
        (null(), null())
    };

    let raw_service = unsafe {
        CreateServiceW(
            manager.0,
            service_name.as_ptr(),
            display_name.as_ptr(),
            SERVICE_START | SERVICE_QUERY_CONFIG | DELETE,
            service_type,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            binary_path.as_ptr(),
            group_ptr,
            null_mut(),
            dependency_ptr,
            null(),
            null(),
        )
    };
    let (service, created) = if raw_service.is_null() {
        let code = unsafe { GetLastError() };
        if code != ERROR_SERVICE_EXISTS {
            return Err(AppError::native(
                format!(
                    "cannot create driver service '{}' for '{}'",
                    options.service_name,
                    canonical_path.display()
                ),
                code,
            ));
        }
        let existing = unsafe {
            OpenServiceW(
                manager.0,
                service_name.as_ptr(),
                SERVICE_START | SERVICE_QUERY_CONFIG,
            )
        };
        if existing.is_null() {
            return Err(AppError::native(
                format!(
                    "service '{}' exists but cannot be opened",
                    options.service_name
                ),
                unsafe { GetLastError() },
            ));
        }
        (ServiceHandle(existing), false)
    } else {
        (ServiceHandle(raw_service), true)
    };

    if !created {
        verify_existing_service(
            &service,
            service_type,
            Path::new(&normalized_path),
            &options.service_name,
        )?;
    }

    match options.kind {
        DriverKind::Kernel => {
            if unsafe { StartServiceW(service.0, 0, null()) } == 0 {
                let code = unsafe { GetLastError() };
                if code == ERROR_SERVICE_ALREADY_RUNNING {
                    println!("Driver '{}' is already loaded.", options.service_name);
                    return Ok(());
                }
                remove_created_service(&service, created, &options.service_name);
                return Err(AppError::native(
                    format!(
                        "Windows could not start driver service '{}' from '{}'",
                        options.service_name,
                        canonical_path.display()
                    ),
                    code,
                ));
            }
            println!(
                "Loaded kernel driver '{}' from '{}'.",
                options.service_name,
                canonical_path.display()
            );
        }
        DriverKind::Minifilter => {
            if let Err(error) = configure_minifilter(options) {
                remove_created_service(&service, created, &options.service_name);
                return Err(error);
            }
            let result = unsafe { FilterLoad(service_name.as_ptr()) };
            if result < 0 {
                let raw = result as u32;
                let win32_code = raw & 0xffff;
                if raw & 0xffff_0000 == 0x8007_0000
                    && matches!(
                        win32_code,
                        ERROR_ALREADY_EXISTS | ERROR_SERVICE_ALREADY_RUNNING
                    )
                {
                    println!("Minifilter '{}' is already loaded.", options.service_name);
                    return Ok(());
                }
                let diagnostic_code = if raw & 0xffff_0000 == 0x8007_0000 {
                    win32_code
                } else {
                    raw
                };
                remove_created_service(&service, created, &options.service_name);
                return Err(AppError::native(
                    format!(
                        "Filter Manager could not load minifilter '{}' at altitude {} from '{}' (HRESULT 0x{:08X})",
                        options.service_name,
                        options.altitude.as_deref().unwrap(),
                        canonical_path.display(),
                        raw
                    ),
                    diagnostic_code,
                ));
            }
            println!(
                "Loaded minifilter '{}' from '{}' at altitude {} (instance '{}').",
                options.service_name,
                canonical_path.display(),
                options.altitude.as_deref().unwrap(),
                options.instance_name.as_deref().unwrap()
            );
        }
    }
    Ok(())
}

fn remove_created_service(service: &ServiceHandle, created: bool, name: &str) {
    if created && unsafe { DeleteService(service.0) } == 0 {
        let code = unsafe { GetLastError() };
        eprintln!(
            "drvload: warning: could not remove newly created service '{name}' after the load failed: Windows error {code} (0x{code:08X}): {}",
            format_error(code)
        );
    }
}

fn verify_existing_service(
    service: &ServiceHandle,
    expected_type: u32,
    expected_path: &Path,
    name: &str,
) -> Result<(), AppError> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
    }
    let code = unsafe { GetLastError() };
    if code != ERROR_INSUFFICIENT_BUFFER || needed == 0 {
        return Err(AppError::native(
            format!("cannot determine the existing configuration of service '{name}'"),
            code,
        ));
    }
    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    let config = storage.as_mut_ptr().cast::<QueryServiceConfigW>();
    if unsafe { QueryServiceConfigW(service.0, config, needed, &mut needed) } == 0 {
        return Err(AppError::native(
            format!("cannot read the existing configuration of service '{name}'"),
            unsafe { GetLastError() },
        ));
    }
    let config = unsafe { &*config };
    let configured_path = unsafe { wide_ptr_to_string(config.binary_path_name) };
    let clean_path = configured_path
        .trim()
        .trim_matches('"')
        .strip_prefix("\\??\\")
        .unwrap_or(configured_path.trim().trim_matches('"'));
    if config.service_type != expected_type
        || !Path::new(clean_path)
            .as_os_str()
            .eq_ignore_ascii_case(expected_path.as_os_str())
    {
        return Err(AppError::new(format!(
            "service name '{name}' already belongs to a different service configuration\n\nExisting type: 0x{:08X}\nExisting path: {}\nRequested type: 0x{expected_type:08X}\nRequested path: {}\n\nChoose a unique name with --name.",
            config.service_type,
            configured_path,
            expected_path.display()
        )));
    }
    Ok(())
}

fn create_registry_key(path: &str) -> Result<RegistryKey, AppError> {
    let path_w = wide(path);
    let mut key = null_mut();
    let mut disposition = 0;
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path_w.as_ptr(),
            0,
            null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_CREATE_SUB_KEY | KEY_WOW64_64KEY,
            null(),
            &mut key,
            &mut disposition,
        )
    };
    if status != 0 {
        return Err(AppError::native(
            format!("cannot create or open registry key HKLM\\{path}"),
            status as u32,
        ));
    }
    Ok(RegistryKey(key))
}

fn set_string(key: &RegistryKey, name: &str, value: &str, key_path: &str) -> Result<(), AppError> {
    let name_w = wide(name);
    let value_w = wide(value);
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name_w.as_ptr(),
            0,
            REG_SZ,
            value_w.as_ptr().cast(),
            (value_w.len() * 2) as u32,
        )
    };
    if status != 0 {
        return Err(AppError::native(
            format!("cannot write registry value HKLM\\{key_path}\\{name}"),
            status as u32,
        ));
    }
    Ok(())
}

fn set_dword(key: &RegistryKey, name: &str, value: u32, key_path: &str) -> Result<(), AppError> {
    let name_w = wide(name);
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name_w.as_ptr(),
            0,
            REG_DWORD,
            (&value as *const u32).cast(),
            4,
        )
    };
    if status != 0 {
        return Err(AppError::native(
            format!("cannot write registry value HKLM\\{key_path}\\{name}"),
            status as u32,
        ));
    }
    Ok(())
}

fn configure_minifilter(options: &Options) -> Result<(), AppError> {
    let instance = options.instance_name.as_deref().unwrap();
    let altitude = options.altitude.as_deref().unwrap();
    let service_base = format!(
        "SYSTEM\\CurrentControlSet\\Services\\{}",
        options.service_name
    );

    // Windows 11 24H2 and later use Parameters\\Instances. Earlier releases use
    // Instances directly below the service. Writing both layouts is harmless and
    // lets one binary support both generations.
    for suffix in ["Instances", "Parameters\\Instances"] {
        let instances_path = format!("{service_base}\\{suffix}");
        let instances = create_registry_key(&instances_path)?;
        set_string(&instances, "DefaultInstance", instance, &instances_path)?;
        let instance_path = format!("{instances_path}\\{instance}");
        let instance_key = create_registry_key(&instance_path)?;
        set_string(&instance_key, "Altitude", altitude, &instance_path)?;
        set_dword(&instance_key, "Flags", 0, &instance_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting() {
        let quote =
            |value: &str| String::from_utf16(&quote_windows_arg(OsStr::new(value))).unwrap();
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("two words"), "\"two words\"");
        assert_eq!(
            quote("C:\\path with space\\"),
            "\"C:\\path with space\\\\\""
        );
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn service_path_removes_extended_prefix() {
        let path = Path::new(r"\\?\C:\drivers\sample.sys");
        let result = service_binary_path(path).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&result[..result.len() - 1]),
            r"C:\drivers\sample.sys"
        );
        assert!(service_binary_path(Path::new(r"\\server\share\sample.sys")).is_err());
    }
}
