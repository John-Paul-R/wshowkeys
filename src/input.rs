use crate::config::{ColorOverride, Config, ModOverride};
use crate::devmgr::DevMgr;
use anyhow::{Context, Result};
use input::event::keyboard::{KeyState, KeyboardEventTrait};
use input::{Libinput, LibinputInterface};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Keypress {
    pub name: String,
    pub utf8: String,
    pub color: ColorOverride,
    pub modifier: ModOverride,
}

impl Keypress {
    /// The text the renderer should display for this key.
    pub fn display_text(&self) -> &str {
        if self.utf8.is_empty() {
            &self.name
        } else {
            &self.utf8
        }
    }

    /// Whether this key is "special" (modifier-like), after applying overrides.
    pub fn is_special(&self) -> bool {
        match self.modifier {
            ModOverride::Force => true,
            ModOverride::Suppress => false,
            ModOverride::Default => self.utf8.is_empty(),
        }
    }
}

pub struct KeyState_ {
    pub keys: Vec<Keypress>,
    pub last_key: Option<Instant>,
}

impl KeyState_ {
    pub fn new() -> Self {
        Self {
            keys: Vec::new(),
            last_key: None,
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }
}

struct DevMgrInterface {
    devmgr: Arc<DevMgr>,
}

impl LibinputInterface for DevMgrInterface {
    fn open_restricted(&mut self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        self.devmgr.open(path.to_str().unwrap_or("")).map_err(|e| {
            eprintln!("devmgr open: {e:#}");
            libc::EACCES
        })
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

pub struct InputHandler {
    libinput: Libinput,
    xkb_context: xkbcommon::xkb::Context,
    xkb_keymap: Option<xkbcommon::xkb::Keymap>,
    xkb_state: Option<xkbcommon::xkb::State>,
}

impl InputHandler {
    pub fn new(devmgr: Arc<DevMgr>) -> Result<Self> {
        let interface = DevMgrInterface { devmgr };
        let mut libinput = Libinput::new_with_udev(interface);
        libinput
            .udev_assign_seat("seat0")
            .map_err(|_| anyhow::anyhow!("failed to assign libinput seat"))?;

        let xkb_context =
            xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);

        Ok(Self {
            libinput,
            xkb_context,
            xkb_keymap: None,
            xkb_state: None,
        })
    }

    pub fn fd(&self) -> RawFd {
        self.libinput.as_raw_fd()
    }

    /// Dispatch pending libinput events and process keyboard events.
    /// Returns true if any new keypresses were added.
    pub fn dispatch(&mut self, key_state: &mut KeyState_, config: &Config) -> Result<bool> {
        self.libinput.dispatch().context("libinput dispatch")?;
        let mut changed = false;

        while let Some(event) = self.libinput.next() {
            match event {
                input::Event::Keyboard(kb_event) => {
                    if let Some(keypress) = self.handle_key(&kb_event, config) {
                        key_state.keys.push(keypress);
                        key_state.last_key = Some(Instant::now());
                        changed = true;
                    }
                }
                _ => {}
            }
        }

        Ok(changed)
    }

    /// Update xkb keymap from a wayland keyboard keymap event.
    pub fn update_keymap(&mut self, fd: OwnedFd, size: u32) {
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            eprintln!("mmap keymap failed");
            return;
        }

        let slice = unsafe { std::slice::from_raw_parts(map as *const u8, size as usize) };
        let map_str = match std::str::from_utf8(slice) {
            Ok(s) => s.trim_end_matches('\0'),
            Err(_) => {
                unsafe { libc::munmap(map, size as usize) };
                eprintln!("keymap is not valid UTF-8");
                return;
            }
        };

        let keymap = xkbcommon::xkb::Keymap::new_from_string(
            &self.xkb_context,
            map_str.to_string(),
            xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1,
            xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
        );

        unsafe { libc::munmap(map, size as usize) };

        if let Some(keymap) = keymap {
            let state = xkbcommon::xkb::State::new(&keymap);
            self.xkb_keymap = Some(keymap);
            self.xkb_state = Some(state);
        } else {
            eprintln!("failed to compile xkb keymap");
        }
    }

    fn handle_key(
        &mut self,
        event: &input::event::keyboard::KeyboardEvent,
        config: &Config,
    ) -> Option<Keypress> {
        let xkb_state = self.xkb_state.as_mut()?;
        let keycode = xkbcommon::xkb::Keycode::new(event.key() + 8);
        let is_press = event.key_state() == KeyState::Pressed;

        xkb_state.update_key(
            keycode,
            if is_press {
                xkbcommon::xkb::KeyDirection::Down
            } else {
                xkbcommon::xkb::KeyDirection::Up
            },
        );

        if !is_press {
            return None;
        }

        let keysym = xkb_state.key_get_one_sym(keycode);
        let name = xkbcommon::xkb::keysym_get_name(keysym);
        let utf8 = xkb_state.key_get_utf8(keycode);

        let utf8 = if utf8.is_empty() || utf8.chars().next().map_or(true, |c| c <= ' ') {
            String::new()
        } else {
            utf8
        };

        let mut keypress = Keypress {
            name: name.clone(),
            utf8,
            color: ColorOverride::Default,
            modifier: ModOverride::Default,
        };

        if let Some(remap) = config.get(&name) {
            if let Some(ref display) = remap.display {
                if keypress.utf8.is_empty() {
                    keypress.name = display.clone();
                } else {
                    keypress.utf8 = display.clone();
                }
            }
            keypress.color = remap.color;
            keypress.modifier = remap.modifier;
        }

        Some(keypress)
    }
}
