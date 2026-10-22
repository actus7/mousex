
## Probe de Acessibilidade (MDM)

Cole o conteúdo de `probe-mac.sh` no Terminal do Mac. Ele recria este crate em `~/mousex-probe`, instala as Xcode CLT e o rustup se faltarem e roda `cargo run --example probe`.
Esperado: `ACESSIBILIDADE: CONCEDIDA`, depois `INJECAO OK`, com o cursor andando 200 px e um "a" digitado no app em foco. Se aparecer `NEGADA`, conceda a permissão ao Terminal em Ajustes > Privacidade e Segurança > Acessibilidade e rode de novo. Se a opção estiver bloqueada pelo MDM, o Mac fica fora do escopo até o TI liberar.
