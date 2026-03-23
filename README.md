# wshowkeys

Displays keypresses on screen on supported Wayland compositors (requires
`wlr_layer_shell_v1` support).

![](https://sr.ht/xGs2.png)

Forked from https://git.sr.ht/~sircmpwn/wshowkeys.

## Installation

### Dependencies

- cairo
- libinput
- pango
- udev
- wayland
- xkbcommon
- Rust toolchain (stable or nightly, 2021 edition)

### Build and install

```
cargo build --release
sudo install -m 4755 -o root target/release/wshowkeys /usr/local/bin/wshowkeys
```

The `install` command copies the binary, sets it owned by root, and sets the
setuid bit in one step. wshowkeys requires setuid root to open input devices;
privileges are dropped immediately after startup.

To uninstall:

```
sudo rm /usr/local/bin/wshowkeys
```

## Usage

```
wshowkeys [-b|-f|-s #RRGGBB[AA]] [-F font] [-t timeout]
    [-a top|left|right|bottom] [-m margin]
```

- *-b #RRGGBB[AA]*: set background color
- *-f #RRGGBB[AA]*: set foreground color
- *-s #RRGGBB[AA]*: set color for special keys
- *-F font*: set font (Pango format, e.g. 'monospace 24')
- *-t timeout*: set timeout before clearing old keystrokes
- *-a top|left|right|bottom*: anchor the keystrokes to an edge. May be specified
  twice.
- *-m margin*: set a margin (in pixels) from the nearest edge
