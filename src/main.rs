mod config;
mod devmgr;
mod input;
mod render;
mod wayland;

use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use wayland_client::Connection;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

const DEFAULT_DEVPATH: &str = "/dev/input/";

fn main() -> Result<()> {
    let devmgr = Arc::new(devmgr::DevMgr::start(DEFAULT_DEVPATH)?);

    let mut foreground: u32 = 0xFFFFFFFF;
    let mut background: u32 = 0x000000CC;
    let mut specialfg: u32 = 0xAAAAAAFF;
    let mut font = String::from("monospace 24");
    let mut timeout_secs: u64 = 1;
    let mut anchor: u32 = 0;
    let mut margin: i32 = 32;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-b" => {
                i += 1;
                background =
                    config::parse_color(args.get(i).context("missing arg for -b")?)?;
            }
            "-f" => {
                i += 1;
                foreground =
                    config::parse_color(args.get(i).context("missing arg for -f")?)?;
            }
            "-s" => {
                i += 1;
                specialfg =
                    config::parse_color(args.get(i).context("missing arg for -s")?)?;
            }
            "-F" => {
                i += 1;
                font = args.get(i).context("missing arg for -F")?.clone();
            }
            "-t" => {
                i += 1;
                timeout_secs = args
                    .get(i)
                    .context("missing arg for -t")?
                    .parse()
                    .context("invalid timeout")?;
            }
            "-m" => {
                i += 1;
                margin = args
                    .get(i)
                    .context("missing arg for -m")?
                    .parse()
                    .context("invalid margin")?;
            }
            a if a.starts_with("-a") => {
                let anchor_str = if a.len() > 2 { &a[2..] } else {
                    i += 1;
                    args.get(i).map(|s| s.as_str()).unwrap_or("")
                };
                match anchor_str {
                    "top" => anchor |= zwlr_layer_surface_v1::Anchor::Top.bits(),
                    "bottom" => anchor |= zwlr_layer_surface_v1::Anchor::Bottom.bits(),
                    "left" => anchor |= zwlr_layer_surface_v1::Anchor::Left.bits(),
                    "right" => anchor |= zwlr_layer_surface_v1::Anchor::Right.bits(),
                    other => bail!("unknown anchor '{other}'"),
                }
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: wshowkeys [-b|-f|-s #RRGGBB[AA]] [-F font] \
                     [-t timeout] [-a top|left|right|bottom] [-m margin]"
                );
                return Ok(());
            }
            other => bail!("unknown argument '{other}'"),
        }
        i += 1;
    }

    let config = config::Config::load();
    let timeout = Duration::from_secs(timeout_secs);

    let mut input_handler = input::InputHandler::new(devmgr)?;
    let mut key_state = input::KeyState_::new();

    let conn = Connection::connect_to_env().context("wayland connect")?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut wsk = wayland::WskState::new();

    display.get_registry(&qh, ());
    event_queue.roundtrip(&mut wsk).context("initial roundtrip")?;

    if wsk.compositor.is_none() {
        bail!("wl_compositor not available");
    }
    if wsk.shm.is_none() {
        bail!("wl_shm not available");
    }
    if wsk.seat.is_none() {
        bail!("wl_seat not available");
    }
    if wsk.layer_shell.is_none() {
        bail!("zwlr_layer_shell_v1 not available");
    }

    event_queue.roundtrip(&mut wsk).context("seat roundtrip")?;

    let surface = wsk.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer_surface = wsk.layer_shell.as_ref().unwrap().get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "showkeys".to_string(),
        &qh,
        (),
    );
    layer_surface.set_size(1, 1);
    layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::from_bits_truncate(anchor));
    layer_surface.set_margin(margin, margin, margin, margin);
    layer_surface.set_exclusive_zone(-1);
    surface.commit();

    wsk.surface = Some(surface);
    wsk.layer_surface = Some(layer_surface);

    let wl_fd = conn.as_fd();
    let input_fd = input_handler.fd();

    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
    use std::os::fd::{AsFd, BorrowedFd};

    while wsk.running {
        event_queue.flush().context("wl flush")?;

        let poll_timeout: PollTimeout = if key_state.keys.is_empty() {
            None::<u16>.into()
        } else if let Some(last) = key_state.last_key {
            let elapsed = last.elapsed();
            if elapsed >= timeout {
                0u16.into()
            } else {
                let ms = (timeout - elapsed).as_millis().min(u16::MAX as u128) as u16;
                ms.into()
            }
        } else {
            None::<u16>.into()
        };

        let mut pollfds = [
            PollFd::new(unsafe { BorrowedFd::borrow_raw(input_fd) }, PollFlags::POLLIN),
            PollFd::new(wl_fd, PollFlags::POLLIN),
        ];

        poll(&mut pollfds, poll_timeout).context("poll")?;

        // Clear expired keys
        if !key_state.keys.is_empty() {
            if let Some(last) = key_state.last_key {
                if last.elapsed() >= timeout {
                    key_state.clear();
                    wsk.set_dirty(&qh);
                }
            }
        }

        // Handle libinput events
        if pollfds[0]
            .revents()
            .map_or(false, |r| r.contains(PollFlags::POLLIN))
        {
            if input_handler.dispatch(&mut key_state, &config)? {
                wsk.set_dirty(&qh);
            }
        }

        // Handle wayland events
        if pollfds[1]
            .revents()
            .map_or(false, |r| r.contains(PollFlags::POLLIN))
        {
            event_queue
                .dispatch_pending(&mut wsk)
                .context("wl dispatch")?;
            conn.prepare_read()
                .map(|guard| guard.read())
                .transpose()
                .context("wl read")?;
            event_queue
                .dispatch_pending(&mut wsk)
                .context("wl dispatch pending")?;
        }

        // Process keymap update if one arrived via wl_keyboard
        if let Some((fd, size)) = wsk.keymap_update.take() {
            input_handler.update_keymap(fd, size);
        }

        // Render if needed
        if wsk.needs_render {
            wsk.render_frame(
                &key_state.keys,
                &font,
                foreground,
                specialfg,
                background,
                &qh,
            );
        }
    }

    Ok(())
}
