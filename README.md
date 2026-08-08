# Calculator

A native GTK4/libadwaita calculator in the style of Google Calculator.

The primary target is a phone running GNOME Shell Mobile (aarch64, ~360–430 px
wide), though it scales up to the desktop fine. App id: `io.matv.Calculator`. It
is a sibling of Warden (Bitwarden), Courier (email) and Jotter (Memos) and is
built the same way — Rust, gtk4-rs 0.11, libadwaita 0.9, a Nix flake with
crane + fenix.

## What it does

Basic and scientific arithmetic with live results as you type, degree/radian
modes, a calculation history persisted between runs, and a memory register
(MS / MR / M+ / M−). The display is a non-editable label — input comes from the
on-screen keypad and a hardware keyboard, so no on-screen keyboard is triggered.

## Build and run

Everything is in the Nix flake; no host Rust toolchain or GTK dev packages
needed.

```sh
# Enter the dev shell (Rust toolchain, gtk4, libadwaita, ...).
# The shellHook compiles the GSettings schema and sets GSETTINGS_SCHEMA_DIR.
nix develop

cargo run
```

Or build the packaged binary:

```sh
nix build            # produces ./result/bin/calculator
./result/bin/calculator
```

To build for aarch64 (the phone):

```sh
nix build .#packages.aarch64-linux.calculator
```

## License

GPL-3.0-or-later.
