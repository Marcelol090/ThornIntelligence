# Thorn Intelligence

**Local-first storage intelligence** — gerenciador desktop para encontrar arquivos e pastas grandes, investigar duplicados verificados por conteúdo e solicitar otimização nativa do Windows em uma interface glassmorphism.

O repositório começou apenas com o README. Esta branch contém a primeira implementação funcional e um plano para evolução; não pressupõe código preexistente do Sparkling.

## Stack

- **Desktop:** Tauri 2 + Rust 2021 (varreduras e comandos do sistema operacional).
- **Interface:** React 19 + TypeScript + Vite 6 + Lucide, CSS glassmorphism responsivo.
- **Arquivos:** `walkdir` para percorrer diretórios sem seguir links simbólicos, `regex` para pesquisa, `blake3` para confirmação de duplicados.
- **Privacidade:** processamento local. Sem backend, conta obrigatória, uploads ou telemetria.
- **Otimização:** `Optimize-Volume` nativo do Windows, sempre iniciado manualmente e respeitando as decisões por tipo de mídia do sistema operacional.

## Iniciar o app no Windows

Pré-requisitos: Node.js 20.19+/22+, Rust stable, Visual Studio C++ Build Tools + Windows SDK e WebView2 (Tauri 2).

```powershell
git clone https://github.com/Marcelol090/ThornIntelligence.git
cd ThornIntelligence
npm install
npm run tauri dev
```

A análise de volumes está separada da pesquisa por metadados. O backend exige autorização por unidade para ações de otimização; não há limpeza ou desfragmentação automática.

`npm run dev` sozinho mostra a interface no navegador, **mas não executa o scanner Rust**. Use `npm run tauri dev` para análise de arquivos e integração com o Windows.

### Funcionalidades presentes (v0.1)

1. Seleção da pasta via diálogo nativo (sem salvar o conteúdo na nuvem).
2. Varredura de tamanho lógico, total de arquivos, maiores diretórios, ranking de arquivos e distribuição por extensão.
3. Filtro por expressão regular no caminho/nome e tamanho mínimo em MB; padrões Regex limitados a 4.096 bytes na busca e no scanner completo.
3a. **Progresso em tempo real e cancelamento cooperativo:** enumeração, hashing e conferência de identidade mostram contagem de arquivos e bytes lidos; cancelamento por operação não devolve resultados parciais e preserva o último relatório.
4. Identificação de duplicados **exatos**: primeiro agrupa por tamanho e depois confirma com hash completo BLAKE3, dentro do limite de leitura. Verifica identidade física com `same-file`, descarta aliases de hardlinks e ignora arquivos vazios nas economias estimadas.
5. Visão de grupos duplicados, hash e caminhos para revisão manual — **sem botão de exclusão**.
6. Análise de fragmentação do volume no Windows e otimização explícita via PowerShell, com política do próprio Windows para HDD/SSD/tiered.
7. Mensagens visíveis para entradas inacessíveis, varreduras truncadas e análise de hash parcial.
8. Diagnóstico Windows **somente leitura** por `Get-Partition`, `Get-Disk`, `Get-Volume` e `Get-StorageReliabilityCounter`, incluindo capacidade, espaço disponível, saúde geral e sensores opcionais de temperatura/desgaste/erros.

**Importante:** tamanhos e economia potencial são valores lógicos; não equivalem necessariamente a blocos físicos livres (hardlinks, compressão, sparse files, deduplicação do filesystem). Não exclua arquivos somente pela semelhança de hash; verifique semântica, acesso e backup.

### Limites operacionais iniciais

- **Sem teto artificial de arquivos** nas análises normais: a enumeração continua até percorrer todos os arquivos acessíveis. `maxFiles` é opcional e só causa truncamento quando solicitado explicitamente. A interface guarda apenas os 300 maiores arquivos e os 500 maiores matches de pesquisa, sem deixar de contabilizar os restantes.
- Orçamento de até **8 GiB de bytes retornados por leituras de conteúdo** para hashing de candidatos duplicados, incluindo tentativas que falhem após leituras parciais. Isso não equivale ao número exato de bytes físicos lidos pelo dispositivo (cache e read-ahead do SO). Fora desse orçamento, o relatório avisa que pode haver mais cópias.
- Exibe até 300 maiores arquivos, 300 diretórios, 300 grupos duplicados e 500 correspondências de busca.
- Leitura direta local, sem indexação persistente nesta versão; pesquisas executam uma nova **enumeração de metadados**, sem refazer hashes BLAKE3, e preservam o relatório de duplicados.
- Não segue symlinks. Erros de acesso são contabilizados.
- Até 1.024 referências por grupo de hash para a identificação de hardlinks. Grupos maiores ou identidades inacessíveis são omitidos da estimativa e sinalizados como análise parcial.
- O módulo de hardlinks usa identificadores de arquivo fornecidos pelo SO; sistemas de arquivos específicos podem ter limitações, portanto nenhuma limpeza destrutiva é autorizada automaticamente.
- O scanner não implementa ainda uma abertura de arquivos livre de condições de corrida (TOCTOU), nem garante que metadados Windows cubram todos os provedores de nuvem. Para segurança máxima, evite escanear pastas sincronizadas sensíveis até validar em Windows.
- O scanner é assíncrono do ponto de vista da UI, com progresso por Tauri Channel e cancelamento cooperativo por sessão. Ainda não implementa snapshot SQLite, retomada de varredura ou indexação incremental.
- A leitura de saúde é Windows-only, e indicadores SMART podem estar ausentes (por modelo, driver, barramento ou privilégios). Um resultado `Healthy` não é garantia de ausência de falhas.
- O módulo de otimização é Windows-only. Não força `-Defrag` em SSD, não eleva privilégios e não agenda tarefas automaticamente. O backend exige análise bem-sucedida da mesma unidade nos últimos 5 minutos, com autorização de uso único; análises simultâneas e operações concorrentes na mesma unidade são bloqueadas.
- Em Windows, arquivos marcados como offline, recall-on-access ou reparse são listados por metadados, mas não têm conteúdo lido para hash; o relatório contabiliza candidatos ignorados. Essa política conservadora pode deixar duplicados não identificados.
- O hash confere tamanho e data de modificação antes/depois da leitura, reduzindo resultados inconsistentes; não elimina condições de corrida do filesystem.
- Funcionalidades ainda planejadas: pré-visualização do impacto, snapshots SQLite incrementais, content-grep e métricas de espaço físico real. A análise de hardlinks já existe e permanece conservadora.

## Comandos de qualidade

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
```

Os testes Rust incluem confirmação de hash para duplicados, Regex inválida, limite de varredura e validação da letra da unidade. CI executa build frontend e testes nativos no Windows; formatação Rust deve ser verificada localmente antes de merge.

Leia [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para módulos, prioridades, política de disco e roadmap.

## Progresso e cancelamento (branch de melhoria)

- A UI cria um `jobId` por varredura ou pesquisa; o backend aceita no máximo uma dessas operações simultaneamente.
- Um `Channel<ScanProgress>` envia apenas **fase**, arquivos processados e bytes retornados por leituras de hash, com emissão limitada a ~4 eventos por segundo (mais mudanças de fase).
- `cancel_scan(jobId)` marca apenas a operação correspondente. A enumeração verifica o cancelamento a cada entrada; o BLAKE3 a cada bloco (64 KiB); hardlinks a cada comparação. A operação cancela de forma cooperativa, sem matar threads abruptamente.
- Cancelamentos rejeitam o relatório parcial, preservam os dados anteriormente apresentados, não apagam arquivos e não afetam a otimização de volumes.
- Não há percentual global verdadeiro antes de conhecer a quantidade de arquivos, portanto a UI não inventa ETA ou progresso percentual.

### Validação obrigatória

```powershell
npm install
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri dev
```

**Bloqueio conhecido:** o GitHub Actions vinha falhando antes de iniciar os jobs. O código desta melhoria precisa passar por esses comandos em Windows antes de uma homologação. O app não executa exclusão automática ou desfragmentação autônoma.

Fontes: [Tauri 2 Channels](https://docs.rs/tauri/latest/tauri/ipc/struct.Channel.html), [Tauri Calling Frontend](https://v2.tauri.app/develop/calling-frontend/), [AssetHoard: lições de IPC em 120.000 arquivos](https://assethoard.com/blog/when-120000-files-meet-tauri).

## Performance: BLAKE3 em duas etapas e exploração cloud-aware (rodada 08/10/2026)

- O scanner faz enumeração e agrupa arquivos por **tamanho**. Para cada grupo de mesmo tamanho, lê no máximo **16 KiB** por candidato para gerar uma assinatura BLAKE3 *parcial*.
- Apenas subgrupos cujo prefixo coincide chegam ao BLAKE3 do **conteúdo completo**. Uma amostra igual não é prova de duplicação e nunca autoriza limpeza. O agrupamento final segue tamanho + hash completo + identidade física, com hardlinks excluídos da estimativa.
- A cota de 8 GiB contabiliza bytes retornados **tanto na amostragem quanto no hashing completo**, inclusive leituras parciais mal sucedidas. Leitura física real depende do cache e do controlador, não equivale a esse contador.
- Os três estágios `scanning`, `fingerprinting` e `hashing` têm contadores separados na interface: arquivos enumerados, candidatos amostrados e candidatos com BLAKE3 completo. O progresso não inventa percentual/ETA.
- O ranking de maiores arquivos limita clones ao top 300 em vez de copiar/ordenar os 250 mil registros.
- Explorador: filtros para **todos**, **metadados remotos/offline** e **redirecionamentos (reparse)** nos top 300. Os marcadores são derivados de atributos de arquivos do Windows, sem abrir conteúdo ou fazer downloads. Diretórios reparse/junction são ignorados para evitar travessia de raízes externas; análise fica explicitamente parcial nesses casos.
- Esses filtros só mostram os 300 maiores itens do relatório, não uma varredura cloud completa. Um atributo ausente não comprova que o arquivo é inteiramente local.

## Glassmorphism nativo no Windows

- A janela Tauri tem `transparent: true`, e o frontend usa `getCurrentWindow().setEffects()` para solicitar **Mica no Windows 11** e **Acrylic como fallback no Windows 10**.
- Após a confirmação da API, apenas as superfícies visuais necessárias se tornam translúcidas; no caso de API indisponível, permanece o tema escuro opaco. O comportamento visual também depende de **Configurações do Windows → Personalização → Cores → Efeitos de transparência** e da versão/build do Windows.
- Atenção: esse blur do DWM atua no fundo da janela; `backdrop-filter` é o desfoque local da camada HTML. Não são a mesma tecnologia e nenhum dos dois deve ser forçado em hardware que não oferecer suporte.

### Validação obrigatória

```powershell
npm install
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri dev
```

Testar: Windows 10 (Acrylic) e Windows 11 (Mica), efeitos de transparência ativados/desativados, redimensionamento e contraste; HDD e NVMe com centenas de milhares de arquivos de diferentes tamanhos; OneDrive Files On-Demand; amostras iguais com finais diferentes; hardlinks; cancelamento e orçamento de leitura. Executar benchmarks **antes/depois** em pasta de teste descartável; não anunciar ganhos numéricos sem medição.

Fontes Exa: [Tauri setEffects](https://v2.tauri.app/reference/javascript/api/namespacewindow/), [Tauri window-vibrancy](https://github.com/tauri-apps/window-vibrancy), [Microsoft File Attribute Constants](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants), [DiskSleuth staged hashing](https://github.com/Swatto86/DiskSleuth).

## Modo rápido (sem BLAKE3) — funcionalidade opt-in

Agora a tela principal começa com **Incluir BLAKE3 desativado**, acelerando o inventário de arquivos grandes, tipos e diretórios sem ler o conteúdo de nenhum arquivo. O relatório marca `hashingSkipped=true`, e a UI mostra **não avaliado** em vez de 0 duplicados/0 bytes de economia (0 seria enganoso).

Ao ativar a opção ou clicar **Executar BLAKE3** na aba de Duplicados, o scanner aplica amostragem de 16 KiB, BLAKE3 completo nos candidatos coincidentes e validação de hardlinks. A configuração é por execução; nenhuma exclusão é feita.

A mudança mantém compatibilidade de API: callers antigos sem `analyzeDuplicates` usam análise completa. O novo frontend solicita explicitamente `analyzeDuplicates: false` por padrão. O teste `metadata_only_mode_skips_content_hash_and_marks_duplicate_metrics_unknown` valida a ausência de leituras de hash e a distinção entre resultados não medidos e zero.

## Correção do limite de 250 mil arquivos (08/10/2026)

O limite era imposto em **dois lugares**: `maxFiles: 250_000` nas chamadas React e `unwrap_or(250_000).clamp(1, 1_000_000)` nos comandos Rust.

- Nas varreduras normais `maxFiles` é omitido. O Rust interpreta `None` como **sem limite numérico de arquivos** e percorre os diretórios acessíveis, inclusive além de 250.000.
- Na **análise rápida**, o scanner não guarda mais uma cópia de todos os caminhos/metadados para hashing: mantém contadores, distribuição por extensão e diretório, além dos rankings limitados a 300 arquivos e 500 correspondências. Consumo de memória cresce ainda com a quantidade de diretórios e tipos, mas não com um vetor de cada arquivo.
- Na **análise completa** com BLAKE3, caminhos elegíveis precisam ficar em memória para as etapas de amostragem/hash. Por isso esse modo continua mais caro em RAM e I/O e não foi substituído por uma promessa irrealista de memória constante. O próximo passo é implementar um spool/indexação persistente de candidatos.
- A busca Regex também percorre todo o escopo sem um limite de arquivos oculto; 500 é somente o limite visual de resultados.
- Caso a API receba `maxFiles: N` explicitamente, ela mantém a possibilidade de amostragem e sinaliza `truncated: true` ao encontrar arquivos além de N. O valor zero é rejeitado.
- Erros de acesso e diretórios virtuais/reparse continuam sendo relatados separadamente. A ausência de um teto não significa acesso universal a arquivos protegidos.

Testes unitários cobrem enumeração rápida completa com ranking limitado, limite explícito e rejeição de zero, além de busca sem truncamento com mais de 500 matches. Validar com >250.000 arquivos reais no Windows após compilar e rodar o Rust/TypeScript.

## Funcionalidade: medir o espaço alocado no Windows (sem ler conteúdos)

Na aba **Explorador**, o botão **Medir espaço em disco** consulta sob demanda o espaço que o filesystem declara como alocado para **até 300 arquivos já exibidos** no ranking. Os dados são apresentados por arquivo, ao lado do tamanho lógico. Nenhum hash BLAKE3 é recalculado e nenhum arquivo é modificado.

**Método e limites:**
- API oficial `GetFileInformationByHandleEx(FileStandardInfo)` (Win32): alocação reportada, tamanho e contagem de hardlinks vêm do **mesmo handle**. A interface identifica referências físicas compartilhadas e não atribui a elas espaço recuperável.
- A consulta é realizada em `spawn_blocking` com progresso/cancelamento por sessão e rejeição explícita de solicitações acima de 300 arquivos. Não percorre novamente a árvore.
- Os arquivos são identificados por caminho absoluto sob a raiz selecionada, com verificação de canonicalização, `symlink_metadata`, atributos offline/reparse e tamanho esperado. Arquivos fora do escopo, modificados, inacessíveis ou virtuais ficam **sem medida**; não se atribui zero artificial.
- O método abre somente um **handle de metadados** (sem permissão de leitura de conteúdo), com `FILE_FLAG_OPEN_REPARSE_POINT` e `FILE_FLAG_OPEN_NO_RECALL`, sem ler bytes dos arquivos. Entradas offline/reparse são excluídas antes da abertura e rechecadas depois. A sinalização `NO_RECALL` não garante o comportamento de todos os provedores cloud, que requer teste Windows/OneDrive real.
- O resultado cobre **apenas os 300 maiores arquivos exibidos**, não o volume inteiro. A contagem de hardlinks pode incluir nomes fora da raiz analisada. Mesmo com contagem 1, dados reflink/deduplicados ou snapshots podem compartilhar blocos; a alocação reportada **não é espaço recuperável**. A quarentena não libera capacidade enquanto preserva o arquivo no mesmo volume.
- O relatório de alocação da sessão é invalidado após nova varredura e não é persistido como um snapshot definitivo.

**Validação exigida:** `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo check --manifest-path src-tauri/Cargo.toml`, `npm run build`; executar em Windows com arquivos esparsos, comprimidos e VHDX locais, com OneDrive Files On-Demand e diretórios redirecionados. Os testes incluem consulta com handle de metadados, detecção de hardlinks (Windows), limite de 300, cancelamento, tamanho desatualizado e arquivos fora da raiz. O teste Windows para hardlinks precisa ser executado em NTFS antes do merge. Não anunciar métricas físicas de volume sem medir o volume inteiro.

**Fontes técnicas obtidas via Exa:** [Microsoft GetCompressedFileSizeW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getcompressedfilesizew), [Microsoft sparse file size](https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-the-size-of-a-sparse-file), [windows-sys Win32 examples](https://docs.rs/crate/zccache/latest/source/src/platform/platform_win/fs/volume.rs).

**Evidências da atribuição por hardlinks (Exa):** [Microsoft FILE_STANDARD_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_standard_info), [Microsoft CreateFileW — acesso zero e abertura de reparse point](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew), [WinDirStat #340 — contagem de hardlinks](https://github.com/windirstat/windirstat/issues/340), [WinDirStat #416 — downloads indesejados do OneDrive](https://github.com/windirstat/windirstat/issues/416).
