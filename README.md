# mcom

A serial terminal for embedded work. It replaces `screen /dev/ttyACM0 115200` and
minicom, and fixes the three things that make them annoying:

- **Colored logs look right.** Output passes through untouched, so ANSI colors
  from ESP-IDF or STM32 logs render exactly as the device sent them.
- **Quitting is one keystroke.** `Ctrl-A q`. No detached sessions, no dialogs.
- **The port is always free afterwards.** On quit, on `SIGTERM`, on `SIGHUP`,
  even on a panic — the handle is closed and the terminal is restored.

Works on macOS and Linux.

## Install

```sh
cargo install --path .
```

The binary is `mcom`; it lands in `~/.cargo/bin`.

## Use

```sh
mcom                       # auto-detect the only USB serial port, 115200 8N1
mcom /dev/ttyACM0          # explicit port
mcom /dev/ttyACM0 -b 9600  # explicit baud rate
mcom --list                # show available ports
mcom --log                 # also write the session to a file
```

## Keys

While connected:

| Key | Action |
| --- | --- |
| `Ctrl-A q` | quit and release the port |
| `Ctrl-A ?` | help |
| `Ctrl-A s` | open the scrollback view |
| `Ctrl-A t` | cycle timestamps: off, relative, absolute, delta |
| `Ctrl-A l` | start or stop logging to a file |
| `Ctrl-A c` | clear the screen |
| `Ctrl-A b` | send a break |
| `Ctrl-A r` | force a reconnect |
| `Ctrl-A i` | session status |
| `Ctrl-A Ctrl-A` | send a literal `Ctrl-A` to the device |

Everything else — `Ctrl-C` included — goes straight to the device.

In the scrollback view (`Ctrl-A s`):

| Key | Action |
| --- | --- |
| `↑` `↓` `PgUp` `PgDn` | scroll |
| `g` / `G` | jump to the top / bottom |
| `/` | search by regex, then `n` and `N` to step through matches |
| `f` | filter lines by regex, live |
| `q` or `Esc` | back to the live stream |
| `Ctrl-A q` | quit |

## Options

`--ts <off\|rel\|abs\|delta>` timestamps (default `rel`) · `--log [PATH]` write a
log file · `--log-format <raw\|clean>` keep escape sequences or strip them
(default `clean`) · `--scrollback <N>` buffer size, default 10000 lines ·
`--no-reconnect` exit instead of waiting for the device · `--strict-port` only
reconnect to the exact same path · `--escape <CHAR>` use another escape key ·
`--echo` echo what you type. See `mcom --help` for the full list.

## Notes

**Unplugging is not fatal.** If the board resets or the cable is pulled, mcom
says so and waits. When the device returns it reconnects and the log continues —
including on macOS, where `usbmodem` devices often come back under a new name.

**macOS:** use `/dev/cu.*`, not `/dev/tty.*`. The `tty` devices block on open
until the modem asserts DCD, which is why a terminal sometimes just hangs. If you
pass a `tty` path, mcom switches to the `cu` twin and tells you.

**Linux:** you need access to the port — `sudo usermod -aG dialout $USER`, then
log out and back in. mcom takes an exclusive `flock`, so a second instance gets a
clear message naming the port instead of `Resource busy`.

**Virtual ports:** pass `-b 0` to leave the line speed alone. Pseudo-terminals
reject baud rate changes, so this is what `socat` and `pty` devices need.
