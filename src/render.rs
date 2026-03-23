use crate::config::ColorOverride;
use crate::input::Keypress;
use anyhow::{Context, Result};
use std::os::fd::{AsFd, OwnedFd};
use wayland_client::protocol::wl_shm;

fn set_source_u32(cr: &cairo::Context, color: u32) {
    cr.set_source_rgba(
        ((color >> 24) & 0xFF) as f64 / 255.0,
        ((color >> 16) & 0xFF) as f64 / 255.0,
        ((color >> 8) & 0xFF) as f64 / 255.0,
        (color & 0xFF) as f64 / 255.0,
    );
}

pub struct PoolBuffer {
    pub buffer: wayland_client::protocol::wl_buffer::WlBuffer,
    pub surface: cairo::ImageSurface,
    pub width: u32,
    pub height: u32,
    _mmap: memmap2::MmapMut,
    pub busy: bool,
}

/// Manages a double-buffered SHM pool for rendering frames.
pub struct BufferPool {
    buffers: [Option<PoolBuffer>; 2],
}

impl BufferPool {
    pub fn new() -> Self {
        Self {
            buffers: [None, None],
        }
    }

    /// Returns a non-busy buffer at the requested size, recreating if dimensions changed.
    pub fn get_buffer(
        &mut self,
        shm: &wl_shm::WlShm,
        width: u32,
        height: u32,
        qh: &wayland_client::QueueHandle<crate::wayland::WskState>,
    ) -> Option<usize> {
        let idx = if self.buffers[0].as_ref().map_or(true, |b| !b.busy) {
            0
        } else if self.buffers[1].as_ref().map_or(true, |b| !b.busy) {
            1
        } else {
            return None;
        };

        let needs_recreate = self.buffers[idx]
            .as_ref()
            .map_or(true, |b| b.width != width || b.height != height);

        if needs_recreate {
            self.buffers[idx] = None;
            self.buffers[idx] = Some(create_buffer(shm, width, height, qh)?);
        }

        if let Some(ref mut buf) = self.buffers[idx] {
            buf.busy = true;
        }

        Some(idx)
    }

    pub fn buffer(&self, idx: usize) -> Option<&PoolBuffer> {
        self.buffers[idx].as_ref()
    }

    pub fn release(&mut self, buffer: &wayland_client::protocol::wl_buffer::WlBuffer) {
        for slot in &mut self.buffers {
            if let Some(ref mut buf) = slot {
                if buf.buffer == *buffer {
                    buf.busy = false;
                    return;
                }
            }
        }
    }
}

fn create_buffer(
    shm: &wl_shm::WlShm,
    width: u32,
    height: u32,
    qh: &wayland_client::QueueHandle<crate::wayland::WskState>,
) -> Option<PoolBuffer> {
    let stride = width * 4;
    let size = (stride * height) as usize;

    let fd = create_shm_file(size).ok()?;
    let mut mmap = unsafe {
        memmap2::MmapOptions::new()
            .len(size)
            .map_mut(&fd)
            .ok()?
    };

    let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();

    let surface = unsafe {
        cairo::ImageSurface::create_for_data_unsafe(
            mmap.as_mut_ptr(),
            cairo::Format::ARgb32,
            width as i32,
            height as i32,
            stride as i32,
        )
        .ok()?
    };

    Some(PoolBuffer {
        buffer,
        surface,
        width,
        height,
        _mmap: mmap,
        busy: false,
    })
}

fn create_shm_file(size: usize) -> Result<OwnedFd> {
    let name = format!(
        "/wl_shm-{:06x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );

    let fd = nix::sys::mman::shm_open(
        name.as_str(),
        nix::fcntl::OFlag::O_RDWR | nix::fcntl::OFlag::O_CREAT | nix::fcntl::OFlag::O_EXCL,
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )
    .context("shm_open")?;

    let _ = nix::sys::mman::shm_unlink(name.as_str());

    nix::unistd::ftruncate(&fd, size as i64).context("ftruncate shm")?;
    Ok(fd)
}

/// Render keys into a cairo recording surface, returning the pixel dimensions.
pub fn measure_and_render(
    keys: &[Keypress],
    font: &str,
    scale: i32,
    foreground: u32,
    specialfg: u32,
    background: u32,
) -> (cairo::RecordingSurface, u32, u32) {
    let recorder =
        cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None).unwrap();
    let cr = cairo::Context::new(&recorder).unwrap();
    cr.set_antialias(cairo::Antialias::Best);

    let mut fo = cairo::FontOptions::new().unwrap();
    fo.set_hint_style(cairo::HintStyle::Full);
    fo.set_antialias(cairo::Antialias::Subpixel);
    cr.set_font_options(&fo);

    cr.set_operator(cairo::Operator::Source);
    set_source_u32(&cr, background);
    cr.paint().unwrap();

    let mut total_w: u32 = 0;
    let mut max_h: u32 = 0;

    for key in keys {
        let special = key.is_special();
        let display = key.display_text();
        let text = if special {
            format!("{display}+")
        } else {
            display.to_string()
        };

        match key.color {
            ColorOverride::Custom(c) => set_source_u32(&cr, c),
            ColorOverride::None => set_source_u32(&cr, foreground),
            ColorOverride::Default => {
                if special {
                    set_source_u32(&cr, specialfg);
                } else {
                    set_source_u32(&cr, foreground);
                }
            }
        }

        cr.move_to(total_w as f64, 0.0);

        let layout = pangocairo::functions::create_layout(&cr);
        let desc = pango::FontDescription::from_string(font);
        layout.set_font_description(Some(&desc));
        layout.set_text(&text);

        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrFloat::new_scale(scale as f64));
        layout.set_attributes(Some(&attrs));
        layout.set_single_paragraph_mode(true);

        pangocairo::functions::update_layout(&cr, &layout);
        let (w, h) = layout.pixel_size();

        pangocairo::functions::show_layout(&cr, &layout);

        total_w += w as u32;
        if (h as u32) > max_h {
            max_h = h as u32;
        }
    }

    (recorder, total_w, max_h)
}

/// Replay a recording surface onto a pool buffer's cairo surface.
pub fn replay_to_buffer(recorder: &cairo::RecordingSurface, target: &cairo::ImageSurface) {
    let cr = cairo::Context::new(target).unwrap();
    cr.save().unwrap();
    cr.set_operator(cairo::Operator::Clear);
    cr.paint().unwrap();
    cr.restore().unwrap();

    cr.set_source_surface(recorder, 0.0, 0.0).unwrap();
    cr.paint().unwrap();

    target.flush();
}
