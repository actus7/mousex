use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

pub fn save(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut created = None;
    for _ in 0..32 {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let pending = path.with_extension(format!("{}.{}.pending.json", std::process::id(), sequence));
        match OpenOptions::new().write(true).create_new(true).open(&pending) {
            Ok(file) => {
                created = Some((pending, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let (pending, mut file) = created.ok_or_else(|| {
        io::Error::new(io::ErrorKind::AlreadyExists, "Limite de colisões de arquivos temporários atingido")
    })?;
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        // std::fs::rename substitui destino existente também no Windows (MoveFileExW).
        // Nunca remover destino antes: falha de substituição preserva layout anterior.
        fs::rename(&pending, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_save_and_failed_replace_preserve_previous_data() {
        let dir = std::env::temp_dir().join(format!("mousex-save-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("layout.json");
        let orphan = path.with_extension(format!("{}.0.pending.json", std::process::id()));
        fs::write(&orphan, b"orphan from interrupted save").unwrap();
        let legacy = path.with_extension("pending.json");
        fs::write(&legacy, b"legacy orphan").unwrap();
        save(&path, b"first").unwrap();
        assert_eq!(fs::read(&orphan).unwrap(), b"orphan from interrupted save");
        assert_eq!(fs::read(&legacy).unwrap(), b"legacy orphan");
        save(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let locked = OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
            assert!(save(&path, b"locked replacement").is_err());
            drop(locked);
            assert_eq!(fs::read(&path).unwrap(), b"second");
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
        }
        save(&path, b"third").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"third");
        assert_eq!(fs::read(&orphan).unwrap(), b"orphan from interrupted save");
        assert_eq!(fs::read(&legacy).unwrap(), b"legacy orphan");
        let blocked = dir.join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("keep"), b"previous").unwrap();
        assert!(save(&blocked, b"new").is_err());
        assert_eq!(fs::read(blocked.join("keep")).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 4);
        fs::remove_dir_all(dir).unwrap();
    }
}
