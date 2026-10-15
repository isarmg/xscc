[![Build Status](https://travis-ci.org/enigo-rs/enigo.svg?branch=master)](https://travis-ci.org/enigo-rs/enigo)
[![Build status](https://ci.appveyor.com/api/projects/status/6cd00pajx4tvvl3e?svg=true)](https://ci.appveyor.com/project/pythoneer/enigo-85xiy)
[![Dependency Status](https://dependencyci.com/github/pythoneer/enigo/badge)](https://dependencyci.com/github/pythoneer/enigo)
[![Docs](https://docs.rs/enigo/badge.svg)](https://docs.rs/enigo)
[![Crates.io](https://img.shields.io/crates/v/enigo.svg)](https://crates.io/crates/enigo)
[![Discord chat](https://img.shields.io/discord/315925376486342657.svg)](https://discord.gg/Eb8CsnN)
[![Gitter chat](https://badges.gitter.im/gitterHQ/gitter.png)](https://gitter.im/enigo-rs/Lobby)


# enigo
Cross platform input simulation in Rust!

- [x] Linux (Wayland) mouse via an installed portal or uinput device
- [x] Linux (Wayland) text via an installed portal or uinput device
- [x] MacOS mouse
- [x] MacOS text
- [x] Win mouse
- [x] Win text
- [x] Custom Parser


```Rust
let mut enigo = Enigo::new();

enigo.mouse_move_to(500, 200);
enigo.mouse_click(MouseButton::Left);
enigo.key_sequence_parse("{+CTRL}a{-CTRL}{+SHIFT}Hello World{-SHIFT}");
```

for more look at examples

Runtime dependencies
--------------------

Linux callers must install an authorized Wayland portal or uinput keyboard and
mouse using `set_custom_keyboard` and `set_custom_mouse` before injecting input.
RustDesk initializes these devices through its Wayland session setup. X11 and
libxdo input backends are not supported.
