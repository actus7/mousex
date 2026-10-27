//! Texto da janela "Sobre", igual no Windows (bandeja) e no Mac/Linux (ícone da barra/bandeja).
//! Versão, autores, licença e URL vêm do `Cargo.toml` de quem chama, que tem o mesmo workspace.

/// Texto do "Sobre" em português ou inglês.
pub fn about_text(portuguese: bool, version: &str, authors: &str, license: &str, repository: &str) -> String {
    let t = |pt: &'static str, en: &'static str| if portuguese { pt } else { en };
    format!(
        "MouseX {version}

{}

{} {authors}
{} {}

{repository}",
        t(
            "Teclado, mouse e área de transferência compartilhados entre computadores Windows, Mac e Linux.",
            "Keyboard, mouse and clipboard shared across Windows, Mac and Linux computers.",
        ),
        t("Autor:", "Author:"),
        t("Licença:", "License:"),
        license.replace(" OR ", t(" ou ", " or ")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LICENSE: &str = "MIT OR Apache-2.0";

    #[test]
    fn sobre_em_ingles_traz_versao_autor_licenca_e_url() {
        let text = about_text(false, "1.2.3", "Alex", LICENSE, "https://example.test/mousex");
        assert_eq!(
            text,
            "MouseX 1.2.3\n\nKeyboard, mouse and clipboard shared across Windows, Mac and Linux computers.\n\nAuthor: Alex\nLicense: MIT or Apache-2.0\n\nhttps://example.test/mousex"
        );
    }

    #[test]
    fn sobre_em_portugues_usa_ou_na_licenca() {
        let text = about_text(true, "1.2.3", "Alex", LICENSE, "https://example.test/mousex");
        assert!(text.contains("Autor: Alex\nLicença: MIT ou Apache-2.0\n"), "{text}");
        assert!(text.starts_with("MouseX 1.2.3\n\nTeclado, mouse e área de transferência"));
    }
}
