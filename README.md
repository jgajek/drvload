# drvload

`drvload` is a small Windows command-line utility for dynamically loading a
kernel driver or file-system minifilter from a `.sys` path. It creates (or safely
reuses) a demand-start driver service, prompts for UAC elevation when necessary,
and reports native Windows errors with contextual troubleshooting hints.

```powershell
cargo build --release

# Kernel driver
.\target\release\drvload.exe C:\drivers\example.sys

# Minifilter (the altitude implies --type minifilter)
.\target\release\drvload.exe --altitude 370030 C:\drivers\example-filter.sys

# Explicit names and load-order group
.\target\release\drvload.exe --type minifilter `
  --name ExampleFilter `
  --instance "ExampleFilter Default Instance" `
  --altitude 370030.5 `
  --load-order-group "FSFilter Activity Monitor" `
  C:\drivers\example-filter.sys
```

Run `drvload --help` for the complete interface.

## Important Windows requirements

- The driver must match the machine architecture and satisfy Windows driver
  signing and Code Integrity policy. The tool does not bypass those controls.
- Production minifilters must use an altitude allocated by Microsoft. If your
  organization owns an integer altitude, it may use appropriate fractional
  altitudes within that allocation.
- The default minifilter load-order group is `FSFilter Activity Monitor` because
  it is convenient for development. Select the group appropriate to the
  minifilter's actual function with `--load-order-group`.
- The service registration is intentionally retained after a successful load,
  so the Service Control Manager continues to own a valid configuration and a
  subsequent invocation can recognize the loaded driver. Use normal Windows
  service/driver administration tools to unload and remove it.
- Windows 11 24H2 moved minifilter instance values under the service's
  `Parameters` key. `drvload` writes both the new and legacy layouts for
  compatibility across supported Windows releases.

## Diagnostics

Errors are written to stderr and include the operation, service/path context,
the decimal and hexadecimal Windows error code, the system error text, and a
targeted hint for common signing, architecture, policy, privilege, and
`DriverEntry` failures. More detail may be available in Event Viewer under the
System and CodeIntegrity logs and in kernel debugger output.

## Scope

This utility performs on-demand loading only. It does not install PnP drivers,
copy driver packages into the Driver Store, bypass signature enforcement, or
modify boot-start configuration.
