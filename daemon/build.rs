// Ícone do MouseX nos executáveis do Windows (Explorer, Gerenciador de Tarefas e bandeja: recurso 1).
use std::{env, path::PathBuf, process::Command};

fn main() {
    let assets = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifesto")).join("../assets");
    // 1 = ícone normal; 2 = cinza (bandeja sem máquinas conectadas ou compartilhamento pausado).
    let icons = [assets.join("mousex.ico"), assets.join("mousex-off.ico")];
    for ico in &icons {
        println!("cargo:rerun-if-changed={}", ico.display());
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let (rc, object) = (out.join("icon.rc"), out.join("icon.o"));
    let script: String = icons
        .iter()
        .enumerate()
        .map(|(i, ico)| format!("{} ICON \"{}\"\n", i + 1, ico.display().to_string().replace('\\', "/")))
        .collect();
    std::fs::write(&rc, script).expect("icon.rc");
    match Command::new("windres").arg(&rc).args(["-O", "coff", "-o"]).arg(&object).status() {
        Ok(status) if status.success() => println!("cargo:rustc-link-arg-bins={}", object.display()),
        // ponytail: sem windres o programa funciona igual, só com o ícone genérico.
        _ => println!("cargo:warning=windres indisponível: executáveis sem ícone do MouseX"),
    }
}
