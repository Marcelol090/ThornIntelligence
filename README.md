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

## Diagnóstico de transparência Windows (08/10/2026)

Ao executar um binário compilado da `main` antiga, o Thorn ainda usa `"backgroundColor": "#080d1a"` e não chama `setEffects`. A janela fica opaca mesmo que os cartões usem CSS `backdrop-filter`.

Na branch deste PR, `tauri.conf.json` usa `transparent: true` e a capability `core:window:allow-set-effects`. O frontend solicita `Effect.Acrylic` primeiro (Windows 10/11) e `Effect.Mica` como alternativa (Windows 11), com `EffectState.Active`. O CSS aplica fundos translúcidos antes da chamada nativa e deixa `html` e `#root` transparentes; fundos escuros sólidos são preservados somente para o modo sem suporte.

O topo da janela agora mostra **Acrylic solicitado** quando a chamada retorna sem erro e **Vidro indisponível** se ocorrer uma exceção. `Acrylic solicitado` não é garantia de que o DWM/WebView2 realmente desenhou transparência (nem de que o fallback usado foi Acrylic); é apenas diagnóstico da API. Detalhes da falha aparecem no tooltip e no console. O status depende da configuração do Windows e do runtime WebView2.

**Roteiro de validação Windows:** (1) `git fetch origin`, `git switch feat/native-glass-adaptive-scanner`, `git pull --ff-only`; (2) `npm install` e `npm run tauri dev` ou rebuild Release após atualizar a branch; (3) verificar Settings → Personalization → Colors → Transparency effects; (4) confirmar Windows 10/11 e runtime WebView2; (5) comparar aparência da janela em frente a um wallpaper colorido e outra janela; (6) abrir DevTools/logs se indicar indisponibilidade; (7) registrar comportamento com múltiplos monitores, maximização, drag e resize. Alguns builds recentes do WebView2/Windows 11 podem deixar Mica opaco independentemente da solicitação correta.

**Fontes pesquisadas com Exa:** [Tauri 2 setEffects](https://v2.tauri.app/reference/javascript/api/namespacewindow/), [window-vibrancy](https://github.com/tauri-apps/window-vibrancy), [MicrosoftEdge/WebView2Feedback #5409](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5409). As alterações ainda requerem compilação e teste visual em Windows e não foram incorporadas à `main`.


## Comparação de duas pastas — Duplicate Decision Engine

A seção **Comparar pastas** funciona mesmo sem uma varredura global prévia:

1. Selecione a **pasta mestre**, cujo conteúdo será apenas consultado.
2. Selecione uma **pasta candidata** separada, que contém possíveis cópias.
3. Execute a comparação. O Rust inventaria as duas pastas e limita a leitura de BLAKE3 aos tamanhos compartilhados entre elas.
4. Inspecione os resultados: cada item confirma hash BLAKE3 e mostra o caminho da cópia candidata e da referência. É possível copiar caminhos para análise manual, mas **não há botão de exclusão**.

Proteções: recusa pastas sobrepostas, ignora caminhos cloud-only/offline/reparse e arquivos vazios, verifica tamanho e mtime durante hashing e compara identidade física para não contar hardlinks como cópias independentes. Resultados de varreduras com erros/limites são identificados como **parciais**. Uma execução suporta no máximo 250 mil arquivos por pasta, 8 GiB de bytes lidos por hash e exibe os 300 maiores matches. A economia apresentada é apenas **lógica potencial**, não quantidade garantida de bytes físicos recuperáveis.

A implementação inclui testes para múltiplas cópias, arquivos com conteúdo diferente e tamanho igual, aliases entre pastas, alias com outra referência independente, cancelamento antes/durante o hash e pastas sobrepostas. Não foi implementada movimentação para quarentena por esta tela; a futura associação com a quarentena exige uma nova aprovação e revalidação antes de qualquer mutação.

Referências: [same-file Handle](https://docs.rs/same-file/latest/same_file/struct.Handle.html), [atributos de arquivos no Windows](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants), [dupeGuru — hardlinks entre sistemas de arquivos](https://github.com/arsenetar/dupeguru/issues/1388).

**Validação de release:** `npm run build`, `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo check --manifest-path src-tauri/Cargo.toml`; repetir em NTFS local, OneDrive Files On-Demand e volumes diferentes com arquivos de teste. Os jobs GitHub Actions precisam iniciar antes de afirmar que o aplicativo foi homologado.


## Índice local e quarentena reversível (em desenvolvimento)

A interface oferece a ação **Atualizar índice SQLite** após escolher e analisar uma pasta. O banco em AppLocalData guarda apenas caminhos, tamanhos, atributos e tempos de modificação; a pesquisa pode consultar a última geração em cache. Cada nova atualização percorre o filesystem e compara metadados, mas NÃO recalcula hashes de conteúdos nem depende apenas de mtime para provar duplicação. Transações SQLite WAL impedem a publicação de um índice parcialmente concluído quando há cancelamento.

Uma seção de **Quarentena segura** permite pré-visualizar um arquivo local por vez e, somente após digitar a confirmação literal, transferi-lo sem sobrescrita para o diretório gerenciado pelo aplicativo. A restauração exige outra confirmação literal. A primeira implementação é apenas Windows, somente arquivos locais normais com um link físico, recusando links, pastas do sistema, OneDrive e volumes diferentes do AppLocalData. **Não existe exclusão permanente automática.**

**Quarentena não libera espaço no mesmo volume e não substitui backup.** A API baseada em caminhos conserva riscos residuais se outro processo modifica as pastas simultaneamente. Não utilizar em ambientes hostis até concluir testes nativos específicos.

Antes de disponibilizar produção, é obrigatório executar o build Rust/Windows, testes de recuperação e testes com arquivos sincronizados. Veja [docs/STORAGE_SAFETY.md](docs/STORAGE_SAFETY.md).


### Indexação de metadados em lotes

O comando Atualizar índice SQLite agora permite todas as entradas acessíveis sem limite numérico automático, e informa quantos lotes de até 1.024 linhas foram gravados. O índice publicado só muda ao completar toda a enumeração; cancelamento descarta a nova geração. Ainda não há retomada real após crash, e o estágio provisório consome espaço adicional. Use pwsh -File scripts/validate-windows.ps1 em Windows para verificar frontend e Rust. A varredura principal sem teto artificial já foi integrada pelo PR #11.


### Pausar e retomar a indexação sem perder a operação ativa

Durante Atualizar índice SQLite, use **Pausar** para interromper a enumeração entre entradas; o contador permanece visível e a tarefa conserva a sessão ativa. Use **Retomar** para continuar a partir do mesmo iterador, sem começar novamente, enquanto o aplicativo continuar aberto. **Cancelar** também funciona durante a pausa: o token de cancelamento acorda o indexador e impede publicar dados incompletos. Só a geração final, totalmente varrida, aparece nas pesquisas. O sistema consulta o token a cada entrada, com atraso de até aproximadamente 50 ms entre verificações (chamadas de disco bloqueadas podem demorar mais). Não é um recurso de retomada após desligamento/reinício: os metadados podem mudar enquanto a máquina está offline e o índice será reenumerado em uma nova tentativa. Consulte docs/ARCHITECTURE.md.

## Navegação hierárquica sob demanda — SQLite e React (08/10/2026)

A tela **Explorador → Árvore indexada por diretório** complementa o antigo ranking dos 300 maiores arquivos. A árvore contém todos os arquivos do snapshot SQLite completo, mas carrega somente os filhos da pasta aberta: 100 por página, 200 como limite absoluto da API. Os resultados usam **paginação keyset** (tipo, tamanho e caminho), pastas antes dos arquivos e ordenação estável por tamanho, com cursor vinculado à geração publicada. Em vez de montar todos os nós React, o componente renderiza apenas as linhas da janela visível.

### Backend e migração

- A indexação acrescenta o caminho pai às tabelas SQLite de arquivos e staging, com migração idempotente para instalações antigas; cria índice composto por raiz, pai, geração, tamanho e caminho para permitir consultas de filhos.
- A nova tabela de diretórios armazena tamanho lógico acumulado e número de arquivos, inclusive pastas vazias. A memória usada para agregar tamanhos cresce com a quantidade de diretórios, não com todos os arquivos. **A árvore e o inventário são publicados na mesma transação**: cancelamento ou erro antes do commit preserva a geração anterior.
- O comando Tauri browse_index_tree consulta apenas o SQLite, com snapshot consistente, valida raiz/pasta/cursores e impõe limite de paginação. Ele não faz varredura do filesystem, não abre arquivos, não calcula BLAKE3 e não hidrata arquivos remotos. O índice continua excluindo entradas reparse/junction que não podem ser enumeradas com segurança.
- Índices gravados por versões anteriores devem ser **atualizados uma vez** para materializar a árvore. O usuário pode fazer isso pelo novo botão Atualizar índice no Explorador.
- Corrigido adicionalmente o retorno de pesquisa SQLite para preencher contentStatus, exigido por FileResult na UI. Estatísticas de diretórios são **lógicas**, não comprovam economia física nem descontam hardlinks.

### Testes / limites

Foram adicionados testes Rust para pastas vazias, agregação de tamanhos, paginação estável em páginas de um item, diretórios fora da raiz, cursores inválidos, cancelamento mantendo o snapshot anterior e invalidação de cursor após nova geração. **A compilação Rust e os ensaios NVMe ainda não foram realizados** devido ao bloqueio anterior dos runners Windows. Medir latência por clique, IOPS, tamanho do WAL e RAM antes/depois com >250 mil arquivos; validar OneDrive, Unicode, diretórios profundos, muitos filhos e grandes snapshots.

Referências pesquisadas com Exa/GitHub: https://github.com/0xf0f/sqlite-file-index ; https://github.com/jpgneves/minidex ; https://github.com/TanStack/virtual ; https://github.com/jameskerr/react-arborist ; https://github.com/Swatto86/AllTheThings ; https://github.com/Ryan-Sayer/strata . Os algoritmos MFT/USN são uma prioridade posterior, pois exigem acesso NTFS apropriado e fallback.

### Reabrir índices existentes sem revarrer a unidade (commit complementar)

O Explorador passa a consultar os **até 100 escopos SQLite publicados mais recentes** e oferece um seletor de pastas já indexadas. A lista é lida em conexão SQLite somente leitura, sem executar WalkDir, BLAKE3, migração, escrita ou atualização de metadados do filesystem. Selecionar um índice abre diretamente sua árvore persistida, mesmo após reiniciar o Thorn, sem criar um novo ScanReport. Os relatórios do ranking de arquivos grandes continuam associados apenas ao escopo de seu último scan — não são misturados com outro índice selecionado.

A listagem indica índices antigos que precisam ser atualizados para materializar a árvore. O usuário pode indexar novamente pelo botão da própria árvore; nenhuma indexação é disparada automaticamente ao reabrir o programa. Existe teste Rust cobrindo índice ausente (sem criar banco) e uma atualização seguida da recuperação do escopo salvo. O snapshot é histórico e pode estar desatualizado em relação ao disco.

Fontes já pesquisadas por Exa/GitHub: file-index e minidex (índices persistentes), TanStack Virtual e react-arborist (árvores responsivas), AllTheThings e Strata (integração futura de MFT/USN).

## NTFS MFT/USN — caminho nativo conservador (PR #16)

Pesquisa conduzida pelo Exa nas APIs oficiais do Windows e repositórios Rust (links ao final). O Thorn passa a tentar um **fast path nativo em volumes NTFS locais com letra e privilégios suficientes**, mantendo o WalkDir como fallback de confiança. Não solicita elevação, não cria/edita o journal e não abre conteúdo de arquivos.

### Enumeração inicial MFT
- A aceleração MFT permanece **experimental e explicitamente opt-in**: defina a variável de ambiente `THORN_EXPERIMENTAL_NTFS_MFT=1` antes de iniciar o Thorn no Windows. Sem essa variável o WalkDir permanece padrão (a verificação USN de intervalo sem mudanças continua disponível se houver permissões).
- A chamada Win32 FSCTL_ENUM_USN_DATA coleta registros USN V2 (FRN, parent FRN, nome e atributos) e reconstrói caminhos sob a raiz selecionada. O acesso nativo limita-se a **2 milhões de registros MFT**; acima disso usa WalkDir para limitar memória.
- A MFT não expõe necessariamente todos os nomes de hardlinks e não fornece o tamanho de cada arquivo em USN_RECORD_V2. Por isso, o caminho nativo **valida a identidade do arquivo e o número de hardlinks** por handle de metadados, rejeita caminhos redirecionados/cloud-offline e usa symlink_metadata para obter tamanho/mtime/atributos. A árvore completa continua publicada somente na transação SQLite final.
- Exige raiz FRN e serial de volume consistentes com o caminho canônico. Ambos também são validados nos checkpoints USN para que uma troca de unidade não revalide indevidamente um snapshot antigo. Versionamento inesperado de USN, registros inválidos, permissões insuficientes e alterações no volume durante a enumeração provocam fallback. A publicação MFT é validada novamente após os metadados; se o USN mudar, o staging é descartado e o WalkDir recomeça. **Em um volume C: muito ativo, inclusive quando o próprio SQLite está no C:, é normal a MFT cair no fallback.** Isso preserva correção, mas não garante aceleração mensurável.

### USN Journal como verificação incremental segura
- Antes do inventário, lê FSCTL_QUERY_USN_JOURNAL e salva **Journal ID e NextUsn inicial**, no mesmo commit da árvore/indexação. O checkpoint inicial, e não o final, garante que mudanças feitas durante a indexação voltem a ser examinadas.
- Em refresh posterior, se o índice e árvore estiverem completos, o journal tiver a mesma identidade e nenhum intervalo tiver sido descartado, a chamada FSCTL_READ_USN_JOURNAL percorre os eventos do intervalo. **Sem eventos relevantes** (sem alterações ou apenas CLOSE), o Thorn retorna as estatísticas do snapshot publicado e evita a reenumeração inteira.
- Qualquer evento de criação, modificação, exclusão, rename, alterações complexas, overflow, truncamento, journal reset, record V3 ou erro de acesso **volta ao inventário completo**. Nesta etapa **não** são aplicados deltas de arquivo ao SQLite via FRN: isso exige uma tabela de identidade estável e tratamento transacional de alterações/renames de diretórios. Não confundir o fast path USN sem mudanças com sincronização incremental completa.
- O relatório informa `indexMethod`: `ntfs_mft`, `usn_unchanged` ou `walkdir`. Os tempos são medidos pelo aplicativo, mas nenhum ganho real foi homologado.

### Validação obrigatória antes de produção
- Windows 10/11 NTFS com e sem execução elevada; HD/SSD SATA/NVMe; volume C: ativo vs volume secundário; hardlinks reais, junctions e OneDrive offline; journal recriado e USN FirstUsn avançado; diretórios com ACL restrita; pastas enormes e cancelamento durante staging/MFT; diferenças de maiúsculas/Unicode; leitura de endereços DOS/extended.
- Conferir número de arquivos e diretórios comparando MFT contra WalkDir, e jamais classificar economia de hardlinks de forma duplicada. Comparar bytes lógicos e agregados da árvore após cada refresh. Compilar no Windows com cargo fmt/test/check e testar as duas versões antes de habilitar esta etapa como padrão em releases.

Referências Exa: [Microsoft FSCTL_ENUM_USN_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data), [USN record formats](https://learn.microsoft.com/en-us/windows/win32/fileio/walking-a-buffer-of-change-journal-records), [journal ID and cursor safety](https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier), [Rust usn-journal-rs](https://github.com/wangfu91/usn-journal-rs), [dowse](https://docs.rs/crate/dowse/latest/source/src/mft.rs). O exemplo do [Bakin's Bits sobre hardlinks](https://bakins-bits.com/2012/06/does-enumerating-files-with-fsctl_enum_usn_data-ever-miss-any-files/) fundamenta a recusa de assumir que FRN equivale a nome de arquivo único.

### Executar um ensaio de MFT no Windows (somente ambiente de teste)

```powershell
# PowerShell em uma cópia local com arquivos de teste; nunca como limpeza
$env:THORN_EXPERIMENTAL_NTFS_MFT = "1"
npm run tauri dev
# Remova a variável para voltar ao comportamento WalkDir padrão:
Remove-Item Env:THORN_EXPERIMENTAL_NTFS_MFT
```

Após **Atualizar índice** confira o método comunicado: `NTFS/MFT verificada` (quando todas as guardas passarem), `USN sem mudanças — sem reenumeração`, ou `WalkDir (fallback seguro)`. A recusa de usar MFT em um volume muito ativo, com hardlinks ou com OneDrive offline é deliberada. Este código não implementa ainda deltas USN complexos de arquivos modificados/renomeados, nem promete velocidades comparáveis a WizTree. Sem testes Windows conclusivos, não o habilite como padrão de produção.
