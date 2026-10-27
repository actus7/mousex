//! Clipboard em stream QUIC unidirecional próprio: imagem grande ou arquivos nunca atrasam teclas nem movimento.
use crate::transfer_progress::{self, Progress};
use mousex_core::MAX_CLIPBOARD_TEXT_BYTES;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Conteúdo de clipboard trafegado entre máquinas; cada plataforma converte do formato nativo.
#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    Text(String),
    Png(Vec<u8>),
    /// Arquivos e pastas copiados: caminhos locais (na origem, os do usuário; no destino, os recebidos).
    Files(Vec<PathBuf>),
}

pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Limites de uma cópia de arquivos: soma dos tamanhos e quantidade de arquivos e pastas.
pub const MAX_FILES_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FILE_ENTRIES: usize = 10_000;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

const KIND_TEXT: u8 = 1;
const KIND_PNG: u8 = 2;
const KIND_FILES: u8 = 3;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10);
/// Transferências guardadas na pasta de recebidos: a nova e a anterior (pode estar sendo colada ou repassada).
const KEPT_TRANSFERS: usize = 2;
/// Pedaço de leitura ao gravar arquivos recebidos: pequeno o bastante para o cancelamento responder logo.
const COPY_CHUNK: usize = 64 * 1024;

/// Lista do que vem no stream, na ordem dos bytes: caminhos relativos com "/", sempre dentro de uma raiz.
#[derive(Serialize, Deserialize, Debug, Default)]
struct Manifest {
    roots: Vec<String>,
    dirs: Vec<String>,
    files: Vec<(String, u64)>,
}

/// Arquivos recebidos ficam na pasta de dados (no Windows, a protegida do agente elevado: um programa comum
/// não planta ali uma junction para o agente gravar ou apagar em outro lugar).
pub fn inbox() -> PathBuf {
    crate::config::data_dir().join("recebidos")
}

/// Cabeçalho: tipo (u8) + tamanho (u32 LE), depois os bytes. Arquivos: o tamanho é o do manifesto,
/// seguido do conteúdo de cada arquivo na ordem do manifesto.
pub async fn send(conn: &quinn::Connection, clip: &Clip) -> Option<()> {
    let (kind, bytes) = match clip {
        Clip::Text(text) => (KIND_TEXT, text.as_bytes()),
        Clip::Png(png) => (KIND_PNG, png.as_slice()),
        Clip::Files(paths) => return send_files(conn, paths.clone()).await,
    };
    let transfer = async {
        let mut stream = conn.open_uni().await.ok()?;
        stream.write_all(&[kind]).await.ok()?;
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await.ok()?;
        stream.write_all(bytes).await.ok()?;
        stream.finish().ok()
    };
    tokio::time::timeout(TRANSFER_TIMEOUT, transfer).await.ok()?
}

/// Tempo para uma cópia de arquivos: folga fixa mais 1 s a cada 2 MB (rede local lenta ainda passa).
fn files_timeout(total: u64) -> Duration {
    Duration::from_secs(30 + total / (2 * 1024 * 1024))
}

async fn send_files(conn: &quinn::Connection, paths: Vec<PathBuf>) -> Option<()> {
    // Lista e abre tudo com os direitos do usuário comum: o agente elevado não lê o que ele não leria
    // (outro programa pode ter posto qualquer caminho no clipboard).
    let prepared =
        tokio::task::spawn_blocking(move || crate::platform::as_standard_user(|| open_all(&paths))).await.ok()?;
    let (manifest, files) = match prepared.and_then(|opened| opened) {
        Ok(found) => found,
        Err(error) => {
            crate::log(&format!("copied files not sent: {error}"));
            return None;
        }
    };
    let body = serde_json::to_vec(&manifest).ok().filter(|b| b.len() <= MAX_MANIFEST_BYTES)?;
    let total: u64 = manifest.files.iter().map(|(_, size)| size).sum();
    let transfer = async {
        let mut stream = conn.open_uni().await.ok()?;
        // Arquivos grandes cedem a vez ao controle e ao clipboard de texto.
        let _ = stream.set_priority(-1);
        stream.write_all(&[KIND_FILES]).await.ok()?;
        stream.write_all(&(body.len() as u32).to_le_bytes()).await.ok()?;
        stream.write_all(&body).await.ok()?;
        for (file, (_, size)) in files.into_iter().zip(&manifest.files) {
            let file = tokio::fs::File::from_std(file);
            // Arquivo que encolheu no meio do caminho: aborta (o destino descarta a cópia incompleta).
            if tokio::io::copy(&mut file.take(*size), &mut stream).await.ok()? != *size {
                return None;
            }
        }
        stream.finish().ok()
    };
    let sent = tokio::time::timeout(files_timeout(total), transfer).await.ok()?;
    if sent.is_some() {
        crate::log(&format!("{} file(s) sent, {} KB", manifest.files.len(), total / 1024));
    }
    sent
}

/// Percorre os caminhos copiados (sem seguir links) e monta o manifesto com os arquivos na ordem de envio.
fn build_manifest(paths: &[PathBuf]) -> io::Result<(Manifest, Vec<PathBuf>)> {
    let mut manifest = Manifest::default();
    let mut sources = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| safe_name(n))
            .ok_or_else(|| io::Error::other(format!("unsupported name: {}", path.display())))?;
        if manifest.roots.contains(&name) {
            return Err(io::Error::other(format!("two items named {name}")));
        }
        manifest.roots.push(name.clone());
        walk(path, &name, &mut manifest, &mut sources)?;
    }
    // Raiz que é link (pulada) não vai: no destino seria um caminho inexistente no clipboard.
    let entries: Vec<String> =
        manifest.dirs.iter().cloned().chain(manifest.files.iter().map(|(path, _)| path.clone())).collect();
    manifest.roots.retain(|root| entries.contains(root));
    if manifest.roots.is_empty() {
        return Err(io::Error::other("nothing to send (only links)"));
    }
    Ok((manifest, sources))
}

/// Manifesto e arquivos já abertos, na ordem do envio.
fn open_all(paths: &[PathBuf]) -> io::Result<(Manifest, Vec<std::fs::File>)> {
    let (manifest, sources) = build_manifest(paths)?;
    let files = sources.iter().map(std::fs::File::open).collect::<io::Result<_>>()?;
    Ok((manifest, files))
}

fn walk(path: &Path, relative: &str, manifest: &mut Manifest, sources: &mut Vec<PathBuf>) -> io::Result<()> {
    if manifest.dirs.len() + manifest.files.len() >= MAX_FILE_ENTRIES {
        return Err(io::Error::other(format!("more than {MAX_FILE_ENTRIES} files")));
    }
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        manifest.dirs.push(relative.into());
        let mut children: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().to_string();
            if !safe_name(&name) {
                return Err(io::Error::other(format!("unsupported name: {}", child.path().display())));
            }
            walk(&child.path(), &format!("{relative}/{name}"), manifest, sources)?;
        }
    } else if meta.is_file() {
        let total = manifest.files.iter().map(|(_, size)| size).sum::<u64>() + meta.len();
        if total > MAX_FILES_BYTES {
            return Err(io::Error::other(format!("more than {} GB", MAX_FILES_BYTES >> 30)));
        }
        manifest.files.push((relative.into(), meta.len()));
        sources.push(path.into());
    }
    Ok(())
}

/// Nome que pode ser criado igual no Windows e no Mac, sem sair da pasta de destino.
fn safe_name(name: &str) -> bool {
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    // O Windows ignora espaços no fim do radical ("NUL .txt" é o dispositivo NUL).
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_uppercase();
    let numbered_device = ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|rest| {
            matches!(rest, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        })
    });
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        && !RESERVED.contains(&stem.as_str())
        && !numbered_device
}

/// Caminho relativo com "/", só com nomes seguros (nada de "..", raiz, unidade ou separador do Windows).
fn safe_relative(relative: &str) -> bool {
    !relative.is_empty() && relative.split('/').all(safe_name)
}

/// Manifesto coerente: nomes seguros, tudo dentro de uma raiz declarada que de fato vem no stream,
/// sem dois caminhos iguais a menos de maiúsculas (um sobrescreveria o outro), dentro dos limites.
fn valid_manifest(manifest: &Manifest) -> bool {
    let root_of = |relative: &str| relative.split('/').next().unwrap_or_default().to_string();
    let total = manifest.files.iter().try_fold(0u64, |sum, (_, size)| sum.checked_add(*size));
    let paths: Vec<&String> = manifest.dirs.iter().chain(manifest.files.iter().map(|(path, _)| path)).collect();
    let unique: std::collections::HashSet<String> = paths.iter().map(|path| path.to_lowercase()).collect();
    let present = |root: &String| paths.contains(&root);
    unique.len() == paths.len()
        && manifest.roots.iter().all(present)
        && !manifest.roots.is_empty()
        && manifest.roots.iter().all(|root| safe_name(root))
        && manifest.dirs.len() + manifest.files.len() <= MAX_FILE_ENTRIES
        && total.is_some_and(|total| total <= MAX_FILES_BYTES)
        && manifest
            .dirs
            .iter()
            .chain(manifest.files.iter().map(|(path, _)| path))
            .all(|path| safe_relative(path) && manifest.roots.contains(&root_of(path)))
}

/// Nome das pastas de transferência: 24 dígitos (ns), hífen e 6 dígitos (contador).
fn is_transfer_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 31 && bytes[24] == b'-' && bytes.iter().enumerate().all(|(i, b)| i == 24 || b.is_ascii_digit())
}

/// Pasta nova para uma transferência; apaga as antigas além das últimas `KEPT_TRANSFERS`.
/// Só pastas de transferência de verdade (nome no formato, sem link) são apagadas.
fn transfer_dir(inbox: &Path) -> io::Result<PathBuf> {
    crate::platform::ensure_inbox(inbox, !cfg!(test))?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(inbox)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()))
        .filter(|entry| entry.file_name().to_str().is_some_and(is_transfer_name))
        .map(|entry| entry.path())
        .collect();
    old.sort();
    for stale in old.iter().rev().skip(KEPT_TRANSFERS - 1) {
        let _ = std::fs::remove_dir_all(stale);
    }
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = inbox.join(format!("{nanos:024}-{:06}", COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Toda recusa ou falha do recebimento passa pelo log, com o motivo (sem isso nada aparece para o usuário).
fn fail(reason: &str) -> Option<Clip> {
    crate::log(reason);
    None
}

async fn read_header(stream: &mut quinn::RecvStream) -> io::Result<(u8, usize)> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await.map_err(|error| io::Error::other(format!("{error:?}")))?;
    Ok((header[0], u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize))
}

pub async fn receive(mut stream: quinn::RecvStream, inbox: &Path, progress: &Progress) -> Option<Clip> {
    let (kind, length) = match tokio::time::timeout(TRANSFER_TIMEOUT, read_header(&mut stream)).await {
        Ok(Ok(header)) => header,
        Ok(Err(error)) => return fail(&format!("incoming clipboard refused: header not read: {error}")),
        Err(_) => return fail("incoming clipboard refused: header timed out"),
    };
    if kind == KIND_FILES {
        return receive_files(stream, length, inbox, progress).await;
    }
    let limit = match kind {
        KIND_TEXT => MAX_CLIPBOARD_TEXT_BYTES,
        KIND_PNG => MAX_CLIPBOARD_IMAGE_BYTES,
        _ => return fail(&format!("incoming clipboard refused: unknown kind {kind}")),
    };
    if length > limit {
        return fail(&format!("incoming clipboard refused: {length} bytes is over the limit of {limit}"));
    }
    let mut bytes = vec![0u8; length];
    match tokio::time::timeout(TRANSFER_TIMEOUT, stream.read_exact(&mut bytes)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return fail(&format!("incoming clipboard refused: body not read: {error:?}")),
        Err(_) => return fail("incoming clipboard refused: body timed out"),
    }
    match kind {
        KIND_TEXT => match String::from_utf8(bytes) {
            Ok(text) => Some(Clip::Text(text)),
            Err(_) => fail("incoming clipboard refused: text is not UTF-8"),
        },
        _ => Some(Clip::Png(bytes)),
    }
}

/// Uma cópia de arquivos chegando por vez (em qualquer conexão): várias de 2 GB ao mesmo tempo encheriam o disco.
static RECEIVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Espaço que sobra no disco depois de uma cópia recebida.
const FREE_MARGIN: u64 = 1024 * 1024 * 1024;

struct ReceivingGuard;

impl Drop for ReceivingGuard {
    fn drop(&mut self) {
        RECEIVING.store(false, std::sync::atomic::Ordering::Release);
    }
}

async fn receive_files(
    mut stream: quinn::RecvStream,
    length: usize,
    inbox: &Path,
    progress: &Progress,
) -> Option<Clip> {
    if length > MAX_MANIFEST_BYTES {
        return fail(&format!("file copy refused: manifest of {length} bytes is over the limit"));
    }
    let mut body = vec![0u8; length];
    match tokio::time::timeout(TRANSFER_TIMEOUT, stream.read_exact(&mut body)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return fail(&format!("file copy failed: manifest not read: {error:?}")),
        Err(_) => return fail("file copy failed: manifest timed out"),
    }
    let Some(manifest) = serde_json::from_slice::<Manifest>(&body).ok().filter(valid_manifest) else {
        return fail("file copy refused: manifest is invalid");
    };
    if RECEIVING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return fail("file copy refused: another one is still arriving");
    }
    let _receiving = ReceivingGuard;
    let total: u64 = manifest.files.iter().map(|(_, size)| size).sum();
    progress.set_total(total);
    progress.set_label(transfer_progress::label(&manifest.roots));
    let inbox = inbox.to_path_buf();
    let dest = match tokio::task::spawn_blocking(move || transfer_dir(&inbox)).await {
        Ok(Ok(dest)) => dest,
        Ok(Err(error)) => return fail(&format!("file copy refused: folder not created: {error}")),
        Err(error) => return fail(&format!("file copy refused: folder task failed: {error}")),
    };
    let remove = |dest: PathBuf| tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dest));
    if crate::platform::free_space(&dest).is_some_and(|free| free < total.saturating_add(FREE_MARGIN)) {
        let _ = remove(dest).await;
        return fail(&format!("file copy refused: not enough disk space for {} MB", total >> 20));
    }
    let outcome = tokio::time::timeout(files_timeout(total), async {
        tokio::select! {
            biased;
            _ = progress.cancelled() => Err("cancelled by the user".to_string()),
            result = write_files(&mut stream, &manifest, &dest, progress) => result,
        }
    })
    .await;
    if progress.is_cancelled() {
        // Avisa o remetente para parar de mandar (senão ele espera o tempo limite à toa).
        let _ = stream.stop(0u32.into());
    }
    let failure = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(_)) if progress.is_cancelled() => Some("file copy cancelled by the user".to_string()),
        Ok(Err(reason)) => Some(format!("file copy failed: {reason}")),
        Err(_) => Some(format!("file copy failed: timed out after {:?}", files_timeout(total))),
    };
    if let Some(reason) = failure {
        let _ = remove(dest).await;
        return fail(&reason);
    }
    crate::log(&format!("{} file(s) received, {} KB", manifest.files.len(), total / 1024));
    Some(Clip::Files(manifest.roots.iter().map(|root| dest.join(root)).collect()))
}

/// Grava pastas e arquivos do manifesto em `dest`; o motivo da falha volta como texto para o log.
async fn write_files(
    stream: &mut quinn::RecvStream,
    manifest: &Manifest,
    dest: &Path,
    progress: &Progress,
) -> Result<(), String> {
    for dir in &manifest.dirs {
        tokio::fs::create_dir_all(dest.join(dir)).await.map_err(|error| format!("folder {dir}: {error}"))?;
    }
    for (relative, size) in &manifest.files {
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| format!("folder of {relative}: {error}"))?;
        }
        let mut file = tokio::fs::File::create(&target).await.map_err(|error| format!("create {relative}: {error}"))?;
        copy_file(stream, &mut file, *size, progress).await?;
    }
    Ok(())
}

/// Copia `size` bytes do stream para o arquivo em pedaços, contando o progresso a cada pedaço.
async fn copy_file(
    stream: &mut quinn::RecvStream,
    file: &mut tokio::fs::File,
    size: u64,
    progress: &Progress,
) -> Result<(), String> {
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut left = size;
    while left > 0 {
        let want = left.min(COPY_CHUNK as u64) as usize;
        // Leitura do tokio (não o read próprio do quinn, que devolve Option).
        let read = AsyncReadExt::read(stream, &mut buffer[..want]).await.map_err(|error| format!("read: {error}"))?;
        if read == 0 {
            return Err("sender stopped before the end of a file".to_string());
        }
        file.write_all(&buffer[..read]).await.map_err(|error| format!("write: {error}"))?;
        left -= read as u64;
        progress.advance(read as u64);
    }
    file.flush().await.map_err(|error| format!("write: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mousex_core::{identity::DeviceIdentity, net};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mousex-clip-{name}-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).expect("pasta");
        dir
    }

    /// Par de conexões QUIC reais (servidor e cliente) em localhost.
    async fn pair() -> (quinn::Connection, quinn::Connection) {
        let root = temp("id");
        let host = DeviceIdentity::load_or_create(&root.join("host")).expect("host");
        let guest = DeviceIdentity::load_or_create(&root.join("guest")).expect("guest");
        let server = net::server_endpoint(&host, 0).expect("server");
        let address: std::net::SocketAddr = ([127, 0, 0, 1], server.local_addr().expect("addr").port()).into();
        let client = net::client_endpoint(&guest).expect("client");
        let (accepted, connected) = tokio::join!(net::accept(&server), net::connect(&client, address));
        (accepted.expect("accept").conn, connected.expect("connect").conn)
    }

    #[test]
    fn nomes_perigosos_sao_recusados() {
        for good in ["a.txt", "pasta/sub/arquivo.bin", "Área de Trabalho/ç.png", ".gitignore"] {
            assert!(safe_relative(good), "{good}");
        }
        for bad in [
            "",
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "a\\b",
            "a//b",
            "con",
            "pasta/NUL.txt",
            "LPT1",
            "nome.",
            "nome ",
            "a\u{0}b",
            "a:b",
            "x?",
        ] {
            assert!(!safe_relative(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn arquivos_e_pastas_chegam_inteiros() {
        let source = temp("origem");
        std::fs::write(source.join("a.txt"), "olá").expect("a");
        std::fs::create_dir_all(source.join("pasta/vazia")).expect("pastas");
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(source.join("pasta/b.bin"), &big).expect("b");
        let (sender, receiver) = pair().await;
        let inbox = temp("recebidos");
        let progress = Progress::default();
        let clip = Clip::Files(vec![source.join("a.txt"), source.join("pasta")]);
        let (sent, received) = tokio::join!(send(&sender, &clip), async {
            receive(receiver.accept_uni().await.expect("stream"), &inbox, &progress).await
        });
        assert!(sent.is_some());
        assert_eq!(progress.snapshot(), (300_004, 300_004), "progresso chega ao total");
        let Some(Clip::Files(paths)) = received else { panic!("nada recebido") };
        let names: Vec<_> = paths.iter().map(|p| p.file_name().expect("nome").to_string_lossy().to_string()).collect();
        assert_eq!(names, ["a.txt", "pasta"]);
        assert!(paths.iter().all(|p| p.starts_with(&inbox)), "tudo dentro da pasta de recebidos");
        assert_eq!(std::fs::read_to_string(&paths[0]).expect("a"), "olá");
        assert_eq!(std::fs::read(paths[1].join("b.bin")).expect("b"), big);
        assert!(paths[1].join("vazia").is_dir(), "pasta vazia também vem");
    }

    #[tokio::test]
    async fn manifesto_com_caminho_perigoso_nao_grava_nada() {
        let (sender, receiver) = pair().await;
        let inbox = temp("recebidos").join("caixa");
        let manifest = br#"{"roots":["x"],"dirs":[],"files":[["../../escapou.txt",3]]}"#;
        let send = async {
            let mut stream = sender.open_uni().await.expect("stream");
            stream.write_all(&[KIND_FILES]).await.expect("tipo");
            stream.write_all(&(manifest.len() as u32).to_le_bytes()).await.expect("tamanho");
            stream.write_all(manifest).await.expect("manifesto");
            stream.write_all(b"abc").await.expect("conteúdo");
            stream.finish().expect("fim");
        };
        let progress = Progress::default();
        let (_, received) = tokio::join!(send, async {
            receive(receiver.accept_uni().await.expect("stream"), &inbox, &progress).await
        });
        assert_eq!(received, None);
        assert!(!inbox.exists(), "nem a pasta de recebidos é criada");
        assert!(!inbox.parent().expect("pai").join("escapou.txt").exists());
    }

    #[tokio::test]
    async fn cancelar_a_copia_apaga_a_pasta_parcial_e_para_o_remetente() {
        let source = temp("origem-cancelar");
        std::fs::write(source.join("grande.bin"), vec![7u8; 8 * 1024 * 1024]).expect("grande");
        let (sender, receiver) = pair().await;
        let inbox = temp("recebidos-cancelar");
        let progress = Progress::default();
        progress.cancel();
        let clip = Clip::Files(vec![source.join("grande.bin")]);
        let (sent, received) = tokio::join!(send(&sender, &clip), async {
            receive(receiver.accept_uni().await.expect("stream"), &inbox, &progress).await
        });
        assert_eq!(received, None);
        assert!(sent.is_none(), "o remetente vê a cópia parar");
        assert_eq!(std::fs::read_dir(&inbox).expect("caixa").count(), 0, "nenhuma pasta parcial sobra");
    }

    #[test]
    fn so_as_duas_ultimas_transferencias_ficam_guardadas() {
        let inbox = temp("recebidos");
        std::fs::create_dir(inbox.join("alheia")).expect("pasta de outro");
        for _ in 0..4 {
            transfer_dir(&inbox).expect("pasta");
        }
        assert_eq!(std::fs::read_dir(&inbox).expect("lista").count(), 3, "duas transferências e a pasta alheia");
        assert!(inbox.join("alheia").exists(), "só apaga pastas de transferência");
    }

    #[test]
    fn dispositivos_do_windows_nao_viram_arquivo() {
        for bad in ["CONIN$", "conout$.txt", "COM0", "LPT0.log", "COM¹", "NUL .txt"] {
            assert!(!safe_name(bad), "{bad}");
        }
    }

    #[test]
    fn manifesto_incoerente_e_recusado() {
        let manifest = |roots: &[&str], files: &[&str]| Manifest {
            roots: roots.iter().map(|r| r.to_string()).collect(),
            dirs: Vec::new(),
            files: files.iter().map(|f| (f.to_string(), 1)).collect(),
        };
        assert!(valid_manifest(&manifest(&["a.txt"], &["a.txt"])));
        assert!(!valid_manifest(&manifest(&["a.txt", "b.txt"], &["a.txt"])), "raiz sem conteúdo");
        assert!(
            !valid_manifest(&manifest(&["p"], &["p/A.txt", "p/a.txt"])),
            "nomes que só mudam a caixa se sobrescreveriam"
        );
    }
}
