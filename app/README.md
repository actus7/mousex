# MouseX desktop

UI independente: React + Vite + Tauri 2. Nenhuma alteração necessária no workspace Rust raiz. `src-tauri/Cargo.toml` declara workspace próprio; não depende de mousex-core nem do transporte em desenvolvimento.

## Executar

```powershell
npm install
npm run dev
npm run build
npm test
npm run tauri dev
npm run tauri build -- --no-bundle
cargo check --manifest-path src-tauri/Cargo.toml
```

Windows: instalar Visual Studio Build Tools 2022 com **Desktop development with C++**, MSVC e Windows SDK; executar em Developer PowerShell. Instalar WebView2 Runtime caso ausente. Linux: dependências nativas Tauri 2 (WebKitGTK 4.1, GTK 3 e build essentials). macOS: Xcode Command Line Tools.

Validação neste ambiente: `npm install` concluído; `npm run build` e três testes Node passaram. `cargo check --offline --manifest-path src-tauri/Cargo.toml` bloqueado por dependência `yoke-derive ^0.8.2` ausente no cache. Tentativa normal baixou dependências, mas encontrou `linker link.exe not found`. Cargo check nativo permanece pendente até instalar/ativar MSVC. Build não abre rede de aplicação; apenas Cargo pode precisar baixar crates. Depois de preencher cache, repetir com `--offline`. Screenshots indisponíveis; revisão visual em execução fica pendente.

## Revisão GNU e persistência (2026-10-21)

`cargo +stable-x86_64-pc-windows-gnu check --manifest-path app/src-tauri/Cargo.toml` passou, execução final em 0.52s. GNU/gcc instalados bastam para este check; bloqueio MSVC acima descreve somente tentativa anterior. Falha reparada: tauri-build exigia `src-tauri/icons/icon.ico`; ícone local adicionado. Nenhuma integração de rede.

`npm --prefix app test`: 3 testes passaram, incluindo limites de dimensão 2147483647/2147483648.

Teste stdlib isolado, executado no Windows com GNU:

```powershell
rustc +stable-x86_64-pc-windows-gnu --test app/src-tauri/src/persistence.rs -o app/src-tauri/target/persistence-check.exe
& app/src-tauri/target/persistence-check.exe
```

Resultado: 1 teste passou. Verifica dois salvamentos consecutivos, substituição com destino bloqueado via share_mode(0), temporário já existente e rename contra diretório. Conteúdo anterior preservado nas falhas; temporário próprio removido em erro. `create_new` impede truncamento de temporário de outra chamada; `write_all` + `sync_all` antecedem rename. Não existe remoção prévia do layout salvo. `std::fs::rename` substitui arquivo existente no Windows, confirmado pelo teste real.

Revisão posterior: temporários agora usam `<layout>.<pid>.<sequência>.pending.json`, com `create_new` e até 32 tentativas em colisão. Temporários órfãos, inclusive formato legado, não bloqueiam novas gravações e não são apagados. Somente temporário criado pela própria operação é limpo em falha. Escritores concorrentes publicam arquivos completos; último rename bem-sucedido prevalece. Teste Windows/GNU passou simulando órfão com colisão real na primeira tentativa, órfão legado, saves repetidos e destino bloqueado preservando conteúdo. Cargo check GNU passou novamente em 0.56s. Durabilidade sob perda de energia e diretórios em filesystem remoto não foram verificadas. Build frontend não repetido nesta revisão: nenhuma fonte frontend alterada. Evidências resumidas em `validation.txt`.

## Componentes

- Primeira execução: escolha explícita de host ou cliente, persistida localmente; troca pelo cabeçalho.
- Host: canvas quadriculado com scroll/zoom, seleção e arraste por pointer (mouse/toque), snap de bordas e alinhamentos, rótulo de máquina/resolução/escala. Coordenadas negativas visíveis. Setas movem 1 px; Shift + setas, 10 px.
- Inspetor: máquina, ID somente leitura, posição, resolução e escala; adicionar/remover telas. IDs novos via UUID.
- Sobreposição destacada em vermelho; botão salvar bloqueado quando inválido. Validação repetida no comando nativo.
- Cliente: endereço manual host:port, validação de porta, área explicando descoberta mDNS futura. Nenhum host fictício.
- Pareamento: diálogo modal nativo HTML, código de seis dígitos, fingerprint, confirmação/recusa; Escape recusa. Aparece via resposta de connect_host ou evento mousex:pairing.
- Barra persistente: desconectado ou máquina, latência e desconectar. Somente eventos reais mudam status; stubs nunca simulam conexão.
- Tema escuro, foco visível, layout adaptável e feedback em região status. Fontes opcionais Google com fallback local; ausência de internet não impede uso.

## Layout confirmado

Formato lido do README raiz, linhas 23–35:

```json
{"monitores":[{"id":"a","machine":"desktop","x":-100,"y":-50,"width":100,"height":100,"scale_fator":1.0}]}
```

Nome é **scale_fator**, não scale_factor. Posições inteiras i32, dimensões positivas limitadas a i32::MAX (2147483647) tanto no frontend quanto na fronteira Rust, escala finita 0.5–4.0, IDs únicos não vazios, máquina não vazia. Bounds semiabertos: bordas podem encostar; sobreposição não. Layout vazio permitido. JSON salvo não inclui metadados de UI. Browser persiste em localStorage; nativo também escreve layout.json em app_config_dir, usando arquivo temporário e rename. Papel e seleção de layout restaurados via localStorage; carregamento nativo alternativo ainda não conectado.

## Contrato provisório / fase 2

Toda rede isolada em `src-tauri/src/bridge.rs`; stubs retornam erro explícito, sem TLS ou sockets. `src/bridge.js` faz somente invoke, ou prévia segura de navegador.

- `connect_host({ address })` deverá iniciar TLS e retornar `{ peer_id, code, fingerprint }`. `code`: string com seis dígitos.
- `confirm_pairing({ peerId, accepted })` deverá aceitar/recusar e aguardar confirmação mútua. UI não marca conexão por confirmação local.
- `disconnect()` deverá liberar teclas/botões e terminar sessão.
- Stream deverá emitir `mousex:connection` com `{ machine, latency_ms }` ou `null`, e `mousex:pairing` com `{ peer_id, code, fingerprint }` inclusive no host.
- mDNS deverá fornecer `discover_hosts -> [{ machine, address, fingerprint }]`; UI atual usa endereço manual, sem inventar descobertas.
- Aplicar layout salvo no serviço ScreenLayout; substituir validação duplicada por tipo do core quando contrato fechar, preservando fronteira validada.
- Adaptar comandos assíncronos enviar_evento/enviar_clipboard e stream nativo. Captura/injeção não pertence ao React.

Não há rede, clipboard ou captura nativa funcionais nesta entrega. Pareamento só aparece quando bridge/evento real fornecer código; não existe modo de pareamento fictício.
