//! Conversão de imagem do clipboard: DIB (CF_DIB) ↔ PNG. Funções puras, testáveis sem Win32.

pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Teto de pixels decodificados (RGBA), evita "bombas" de descompressão.
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
const HEADER: usize = 40;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    Text(String),
    Png(Vec<u8>),
    /// Arquivos e pastas copiados (caminhos absolutos locais).
    Files(Vec<std::path::PathBuf>),
}

/// PNG acima do limite é ignorado sem erro visível.
pub fn clip_from_png(png: Vec<u8>) -> Option<Clip> {
    (png.len() <= MAX_CLIPBOARD_IMAGE_BYTES).then_some(Clip::Png(png))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// Aceita DIB 24/32 bpp, BI_RGB ou BI_BITFIELDS (máscaras BGRA padrão), top-down ou bottom-up.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let header_size = le_u32(dib, 0)? as usize;
    let width = le_u32(dib, 4)? as i32;
    let height = le_u32(dib, 8)? as i32;
    let bits = le_u16(dib, 14)?;
    let compression = le_u32(dib, 16)?;
    if header_size < HEADER || width <= 0 || height == 0 || !matches!(bits, 24 | 32) {
        return None;
    }
    let masks = match compression {
        BI_RGB => 0,
        BI_BITFIELDS if header_size == HEADER => 12,
        BI_BITFIELDS => 0,
        _ => return None,
    };
    let (width, rows) = (width as usize, height.unsigned_abs() as usize);
    let bytes_per_pixel = bits as usize / 8;
    let stride = (width * bytes_per_pixel).div_ceil(4) * 4;
    if width.checked_mul(rows)?.checked_mul(4)? > MAX_DECODED_BYTES {
        return None;
    }
    let pixels = dib.get(header_size + masks..)?;
    if pixels.len() < stride * rows {
        return None;
    }
    let mut rgba = Vec::with_capacity(width * rows * 4);
    for row in 0..rows {
        let source = if height > 0 { rows - 1 - row } else { row };
        let line = &pixels[source * stride..source * stride + width * bytes_per_pixel];
        for pixel in line.chunks_exact(bytes_per_pixel) {
            let alpha = if bytes_per_pixel == 4 { pixel[3] } else { 255 };
            rgba.extend([pixel[2], pixel[1], pixel[0], alpha]);
        }
    }
    // DIB 32bpp costuma vir com alfa zerado: imagem inteira transparente é tratada como opaca.
    if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, rows as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// PNG → DIB 32bpp BI_RGB bottom-up (BGRA), o formato que mais aplicativos aceitam.
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (info_width, info_height) = (reader.info().width as usize, reader.info().height as usize);
    if info_width.checked_mul(info_height)?.checked_mul(4)? > MAX_DECODED_BYTES {
        return None;
    }
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).ok()?;
    // Dimensões do quadro decodificado (APNG pode diferir do cabeçalho); nunca confiar no cabeçalho para fatiar.
    let (width, height) = (frame.width as usize, frame.height as usize);
    let data = &buffer[..frame.buffer_size()];
    let rgba: Vec<[u8; 4]> = match frame.color_type {
        png::ColorType::Rgba => data.as_chunks::<4>().0.to_vec(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    if rgba.len() != width.checked_mul(height)? {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER + width * height * 4);
    out.extend((HEADER as u32).to_le_bytes());
    out.extend((width as i32).to_le_bytes());
    out.extend((height as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]);
    for row in (0..height).rev() {
        for p in &rgba[row * width..(row + 1) * width] {
            out.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    Some(out)
}
