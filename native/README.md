# mousex-native

Crate independente, Windows somente. `[workspace]` impede integração implícita ao workspace pai. Sem Linux/macOS, imagens, rede ou dependências no frontend.

## Uso e validação (PowerShell)

```powershell
cargo +stable-x86_64-pc-windows-gnu test
cargo +stable-x86_64-pc-windows-gnu check
cargo +stable-x86_64-pc-windows-gnu clippy --all-targets -- -D warnings
cargo +stable-x86_64-pc-windows-gnu run
```

CLI apenas enumera monitores; não instala hooks, não injeta entrada e não lê/escreve clipboard.

## API e limites

- `monitors`: retângulos em coordenadas do desktop e indicador principal. Coordenadas podem ser negativas; virtualização DPI depende do contexto DPI do processo chamador.
- `Hooks::start`: singleton por processo, teclado e mouse low-level em thread própria com message loop. Retorna receiver de eventos. Sempre chama `CallNextHookEx`, sem supressão. `shutdown` envia `WM_QUIT`, remove hooks e aguarda thread; Drop tenta mesmo encerramento. Canal não limitado: consumir eventos continuamente para evitar crescimento de memória. Callbacks só decodificam/publicam; não executar trabalho pesado na thread dos hooks.
- `InputState`: acompanha pressionamentos e gera liberações idempotentes. `Injector` registra somente entradas enviadas com sucesso; `release_all` tenta todas as liberações, retém falhas para nova tentativa e retorna primeiro erro. Drop faz tentativa final, sem garantia após término forçado do processo.
- `Injector::send`: `SendInput` com marcador `dwExtraInfo`; hooks descartam somente esse marcador para evitar eco. Não é mecanismo de autenticação: outro processo pode copiar marcador. Entradas de outros injetores continuam visíveis. Teclado usa virtual keys (1–254), sem composição Unicode/IME. Mouse observado usa posições absolutas do desktop; `Event::Move` enviado significa deslocamento relativo, sujeito à aceleração do Windows. Adaptador deve converter explicitamente, não reenviar posição observada como deslocamento.
- Clipboard: somente `CF_UNICODETEXT`, máximo 1.048.576 unidades UTF-16 incluindo terminador. Rejeita NUL interno, UTF-16 inválido e falta de terminador. Janela message-only fornece owner válido; GlobalLock/GlobalUnlock balanceados. Handle transferido ao Windows apenas após `SetClipboardData` bem-sucedido; em falha, memória liberada localmente. Clipboard ocupado retorna erro sem retry. Falha após `EmptyClipboard` pode deixar clipboard vazio; API Win32 não oferece transação/rollback. Conteúdo nunca é alterado implicitamente.

## Windows, UAC e desktop seguro

Hooks e entrada limitados à sessão/desktop interativo atual. Não capturam nem controlam desktop seguro (UAC, logon, Ctrl+Alt+Del), outras sessões ou desktops. UIPI impede `SendInput` contra processos com integridade maior; valor retornado/last-error não identifica UIPI de forma confiável. Não há bypass de UAC, elevação automática, serviço ou UIAccess. Execute no nível de integridade adequado ao aplicativo autorizado. Não há garantia de captura se Windows remover hook por timeout, desktop mudar ou processo encerrar.

Testes não instalam hooks, não enviam entradas e não acessam clipboard. Cobrem estado, mapeamento de botões/roda e marcador anti-loop. Teste interativo de hooks, permissões/UIPI e clipboard fica para integração controlada pelo chamador.
