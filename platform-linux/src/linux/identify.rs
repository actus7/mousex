//! Identificar telas no X11: uma janela sem gerenciador (override-redirect) por monitor, com o número desenhado
//! em retângulos brancos. Região de entrada vazia (XShape) faz o clique passar; sem foco por ser override-redirect.
use super::{other, wayland_session};
use crate::{Rect, number_cell, number_pixels};
use std::{io, thread, time::Duration, time::Instant};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT,
    connection::Connection,
    protocol::{
        Event,
        shape::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const SHOW_FOR: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(10);
/// Opacidade pedida ao compositor (80%); sem compositor a janela fica opaca.
const OPACITY: u32 = 0xCCCC_CCCC;

/// Mostra os números numa thread própria e volta na hora. A conexão e as janelas são criadas aqui para que
/// erros (sem X11, sem RandR) cheguem a quem chamou.
pub fn show_identify(screens: &[(Rect, u32)]) -> io::Result<()> {
    // ponytail: no Wayland não há como posicionar janela de cliente; só o log avisa.
    if wayland_session() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Wayland não permite posicionar janelas"));
    }
    if screens.is_empty() {
        return Ok(());
    }
    let (conn, screen_num) = x11rb::connect(None).map_err(other)?;
    let screen = conn.setup().roots.get(screen_num).ok_or_else(|| other("X11 screen missing"))?;
    let (root, black, white) = (screen.root, screen.black_pixel, screen.white_pixel);
    let opacity = conn.intern_atom(false, b"_NET_WM_WINDOW_OPACITY").map_err(other)?.reply().map_err(other)?.atom;
    let mut overlays = Vec::with_capacity(screens.len());
    for &(screen, number) in screens {
        let window = conn.generate_id().map_err(other)?;
        let cell = number_cell(screen, number);
        let (width, height, pixels) = number_pixels(number, cell);
        let (left, top) = ((screen.width - width) / 2, (screen.height - height) / 2);
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            screen.x as i16,
            screen.y as i16,
            screen.width as u16,
            screen.height as u16,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &xproto::CreateWindowAux::new()
                .background_pixel(black)
                .override_redirect(1)
                .event_mask(xproto::EventMask::EXPOSURE),
        )
        .map_err(other)?;
        conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, xproto::ClipOrdering::UNSORTED, window, 0, 0, &[])
            .map_err(other)?;
        conn.change_property32(xproto::PropMode::REPLACE, window, opacity, xproto::AtomEnum::CARDINAL, &[OPACITY])
            .map_err(other)?;
        conn.map_window(window).map_err(other)?;
        let rects = pixels
            .into_iter()
            .map(|p| xproto::Rectangle {
                x: (left + p.x) as i16,
                y: (top + p.y) as i16,
                width: p.width as u16,
                height: p.height as u16,
            })
            .collect();
        overlays.push((window, rects));
    }
    let gc = conn.generate_id().map_err(other)?;
    conn.create_gc(gc, root, &xproto::CreateGCAux::new().foreground(white)).map_err(other)?;
    conn.flush().map_err(other)?;
    thread::Builder::new().name("mousex-identify".into()).spawn(move || paint(conn, gc, overlays))?;
    Ok(())
}

/// Desenha cada janela quando ela é exposta e as destrói ao fim do tempo. Se a conexão cair, as janelas somem junto.
fn paint(conn: RustConnection, gc: xproto::Gcontext, overlays: Vec<(xproto::Window, Vec<xproto::Rectangle>)>) {
    let end = Instant::now() + SHOW_FOR;
    while Instant::now() < end {
        while let Ok(Some(event)) = conn.poll_for_event() {
            if let Event::Expose(expose) = event
                && expose.count == 0
                && let Some((window, rects)) = overlays.iter().find(|(window, _)| *window == expose.window)
            {
                let _ = conn.poly_fill_rectangle(*window, gc, rects);
            }
        }
        if conn.flush().is_err() {
            return;
        }
        thread::sleep(POLL);
    }
    for (window, _) in &overlays {
        // Conexão pode ter caído no meio: nada a fazer além de sair.
        let _ = conn.destroy_window(*window);
    }
    let _ = conn.flush();
}
