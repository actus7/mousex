//! Estado da ligação com o servidor, lido pelo ícone da barra (Mac) e da bandeja (Linux).
//! Quem grava é o loop do cliente; o ícone só lê. Sem ícone (Windows) só grava, sem efeito.
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Link {
    /// Sem sessão: procurando o servidor, ou esperando para tentar de novo.
    #[default]
    Searching,
    /// Sessão ativa com este servidor; vazio enquanto ele não informou o nome (versões antigas).
    Connected(String),
}

// Sem envenenamento útil: o valor é só um enum, então um pânico com o lock não o deixa inconsistente.
static LINK: Mutex<Link> = Mutex::new(Link::Searching);

pub fn set(link: Link) {
    if let Ok(mut current) = LINK.lock() {
        *current = link;
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn get() -> Link {
    LINK.lock().map(|current| current.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comeca_procurando_e_volta_a_procurar_depois_da_sessao() {
        assert_eq!(Link::default(), Link::Searching);
        set(Link::Connected("win2".into()));
        set(Link::Searching);
        assert_eq!(*LINK.lock().expect("estado"), Link::Searching);
    }
}
