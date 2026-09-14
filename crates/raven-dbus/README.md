# raven-dbus

DBus service interface for RavenFileManager.

Exposes file manager functionality over the D-Bus session bus for external scripting and integration.

## Components

- **DbusService** - Registers on the session bus, dispatches incoming method calls to AppCommands
- **DbusInterface** - Defines the available methods and signals

## Bus Name

Default: `com.ravenfilemanager.Raven` (configurable in `config.toml`), served
when `dbus.enabled` is set. The object path follows the name
(`/com/ravenfilemanager/Raven`) and the interface is
`com.ravenfilemanager.Raven1`: `Navigate`, `GetCurrentPath`, `GetSelection`,
`CopyFiles`, `MoveFiles`, `DeleteFiles`, `TrashFiles`, `Search`,
`TriggerAutomationRule`, and the `DirectoryChanged` / `OperationCompleted`
signals. Paths must be absolute. With several windows open the first owns the
name and the others queue for it.

```sh
gdbus call --session --dest com.ravenfilemanager.Raven \
  --object-path /com/ravenfilemanager/Raven \
  --method com.ravenfilemanager.Raven1.GetSelection
```

## Tests

29 tests covering service initialization, method dispatch, and event handling.
