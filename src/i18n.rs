//! Idioma da interface: português para sistemas em português, inglês para os demais.
//! `MOUSEX_LANG=pt|en` força um idioma (testes, suporte).
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pt,
    En,
}

impl Lang {
    /// "pt", "pt-BR", "pt_PT.UTF-8"… → português; qualquer outro → inglês.
    pub fn from_locale(locale: &str) -> Self {
        let language = locale.split(['-', '_', '.']).next().unwrap_or_default();
        if language.eq_ignore_ascii_case("pt") { Lang::Pt } else { Lang::En }
    }

    pub fn pick<T>(self, pt: T, en: T) -> T {
        match self {
            Lang::Pt => pt,
            Lang::En => en,
        }
    }
}

/// Idioma do sistema (lido uma vez).
pub fn lang() -> Lang {
    static LANG: OnceLock<Lang> = OnceLock::new();
    *LANG.get_or_init(|| {
        let locale = std::env::var("MOUSEX_LANG").ok().or_else(sys_locale::get_locale).unwrap_or_default();
        Lang::from_locale(&locale)
    })
}

/// Texto no idioma do sistema.
pub fn tr(pt: &'static str, en: &'static str) -> &'static str {
    lang().pick(pt, en)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idioma_vem_do_codigo_de_localidade() {
        for pt in ["pt-BR", "pt_BR.UTF-8", "pt-PT", "pt"] {
            assert_eq!(Lang::from_locale(pt), Lang::Pt, "{pt}");
        }
        for en in ["en-US", "de-DE", "es", "", "ptx"] {
            assert_eq!(Lang::from_locale(en), Lang::En, "{en}");
        }
    }

    #[test]
    fn escolhe_o_texto_do_idioma() {
        assert_eq!(Lang::Pt.pick("Sim", "Yes"), "Sim");
        assert_eq!(Lang::En.pick("Sim", "Yes"), "Yes");
    }
}
