# raven-dbus

DBus service interface for RavenFileManager.

Exposes file manager functionality over the D-Bus session bus for external scripting and integration.

## Components

- **DbusService** - Registers on the session bus, dispatches incoming method calls to AppCommands
- **DbusInterface** - Defines the available methods and signals

## Bus Name

Default: `com.ravenfilemanager.Raven` (configurable in `config.toml`).

## Tests

29 tests covering service initialization, method dispatch, and event handling.
