//! Partes comuns da instalação de cliente no Mac e no Linux: sem administrador, tudo na pasta do usuário.
use crate::config::{Config, Role, data_dir};
use std::{error::Error, path::Path, path::PathBuf};

pub const CLI: &str = "mousex";
pub const AGENT: &str = "mousex-agent";

/// `--role client [--machine NOME]`; devolve o nome da máquina. O Mac usa isto; o Linux tem o próprio parser.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn machine_from_args(args: &[String], default: String, os: &str) -> Result<String, Box<dyn Error>> {
    let mut machine = default;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next().map(String::as_str)) {
            ("--role", Some("client")) => {}
            ("--role", Some(_)) => return Err(format!("on {os} only the client role is available").into()),
            ("--machine", Some(name)) if !name.trim().is_empty() => machine = name.trim().into(),
            _ => return Err("uso: mousex install --role client [--machine NOME]".into()),
        }
    }
    Ok(machine)
}

/// Copia `mousex` e `mousex-agent` da pasta deste executável para `bin`.
pub fn copy_binaries(bin: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(bin)?;
    let source = std::env::current_exe()?.parent().map(PathBuf::from).ok_or("executable folder unavailable")?;
    for name in [CLI, AGENT] {
        let (from, to) = (source.join(name), bin.join(name));
        if from != to {
            // Remover antes de copiar: sobrescrever um binário em uso falha no Linux (ETXTBSY) e, no Mac,
            // invalida a assinatura em cache.
            let _ = std::fs::remove_file(&to);
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Grava a configuração de cliente mantendo pareamento e preferências de uma instalação anterior.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn save_config(machine: &str) -> Result<(), Box<dyn Error>> {
    save_role(Role::Client, machine)
}

/// Saves the role. A client keeps its previous pairing; a server has none.
pub fn save_role(role: Role, machine: &str) -> Result<(), Box<dyn Error>> {
    let data = data_dir();
    let previous = Config::load(&data);
    let config = Config {
        role,
        machine: machine.into(),
        server_fingerprint: (role == Role::Client)
            .then(|| previous.as_ref().and_then(|c| c.server_fingerprint.clone()))
            .flatten(),
        auto_update: previous.as_ref().is_none_or(|c| c.auto_update),
        remap_modifiers: previous.as_ref().is_none_or(|c| c.remap_modifiers),
    };
    Ok(config.save(&data)?)
}

/// Configuração e as últimas linhas do log; o estado do serviço vem de quem chama.
pub fn print_status(service: &str) {
    match Config::load(&data_dir()) {
        Some(config) => println!(
            "role: {:?}, computer: {}, paired server: {}",
            config.role,
            config.machine,
            config.server_fingerprint.as_deref().map_or("none", |fp| &fp[..16.min(fp.len())])
        ),
        None => println!("not configured"),
    }
    println!("{}", service.trim());
    if let Ok(log) = std::fs::read_to_string(data_dir().join("mousex.log")) {
        let lines: Vec<&str> = log.lines().collect();
        for line in &lines[lines.len().saturating_sub(15)..] {
            println!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nome_da_maquina_vem_do_argumento_ou_do_padrao() {
        assert_eq!(machine_from_args(&args(&["--role", "client"]), "host".into(), "Linux").unwrap(), "host");
        assert_eq!(
            machine_from_args(&args(&["--role", "client", "--machine", " note "]), "host".into(), "Linux").unwrap(),
            "note"
        );
    }

    #[test]
    fn papel_de_servidor_e_argumento_estranho_sao_recusados() {
        let error = machine_from_args(&args(&["--role", "server"]), "h".into(), "Linux").unwrap_err();
        assert!(error.to_string().contains("only the client"));
        assert!(machine_from_args(&args(&["--machine"]), "h".into(), "Linux").is_err());
        assert!(machine_from_args(&args(&["--foo", "bar"]), "h".into(), "Linux").is_err());
    }
}
