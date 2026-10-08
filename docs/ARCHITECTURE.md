# Arquitetura — Thorn Intelligence

## Proposta

Superar um limpador de disco básico com um **observatório local de armazenamento**, evidências verificáveis e ações seguras. O código e as métricas do Sparkling não estão neste repositório: qualquer comparação quantitativa exigirá os builds/dados do produto anterior.

## Fluxo do MVP

```text
React/Tauri Window
 ├── seletor de pasta (dialog:allow-open)
 ├── scan_path({root,regex,minSizeBytes,maxFiles})
 │    └── Rust spawn_blocking
 │         ├── WalkDir (follow_links=false)
 │         ├── metadados e somatórios de diretórios
 │         ├── Regex de caminho e filtro de tamanho
 │         ├── grupos candidatos por tamanho
 │         ├── guarda contra atributos Windows offline/reparse/recall
 │         ├── BLAKE3 de conteúdo (até 8 GiB lidos; metadados revalidados)
 │         └── identidade do arquivo (same-file) para descartar hardlinks
 ├── disk_health({drive}) — Windows Storage read-only
 └── optimize_volume({drive,execute})
      ├── gate Rust por unidade (análise aprovada, TTL 5 min, uso único)
      └── Windows PowerShell > Optimize-Volume
           ├── execute=false: -Analyze
           └── execute=true: modo automático do Windows
```

O frontend recebe relatórios serializados (camelCase) e apresenta **apenas informações medidas**. Dados não são enviados para serviços externos.

### Segurança operacional

- Não implementar exclusão silenciosa, deduplicação via hardlink ou limpeza de cache sem revisão e opt-in.
- Exigir confirmação explícita para toda operação modificadora. Modo análise nunca otimiza. O backend mantém estado de análise por unidade com expiração e consumo único, sem confiar exclusivamente na UI.
- Usar `Command::new` com argumentos fixos, aceitando apenas uma letra ASCII de unidade.
- Deixar o sistema operacional identificar HDD/SSD/tiers; não usar `-Defrag` indiscriminadamente.
- Respeitar privilégios do usuário e mostrar falhas de permissão.
- Não seguir junctions/symlinks durante a varredura nem atravessar outros destinos intencionalmente por links.
- Não afirmar economia física: APFS/NTFS sparse/reparse/hardlinks/compressão alteram cálculo real.
- Descartar hardlink aliases na estimativa com verificação de identidade via `same-file`; impor limite de descritores e marcar grupos não verificados como parciais.
- Antes de ler conteúdo no Windows, excluir do hash arquivos com atributos offline, recall-on-open, recall-on-data-access e reparse; revalidar metadados após a leitura. O relatório explicita candidatos omitidos. TOCTOU e outros provedores de nuvem ainda requerem testes específicos.
- Não declarar duplicação por nome/tamanho/partial hash apenas: o candidato exige digest integral; hashes são uma evidência forte, não verificação byte a byte para situações adversariais.
- Sem comandos genéricos digitados pelo usuário, sem execução remota.

### Arquivos de origem

- `src/App.tsx`: navegação, estados, busca, tabelas, ações explícitas, comunicação Tauri.
- `src/styles.css`: tokens visuais dark/glass, backdrop-filter, breakpoints, foco visível e reduced motion.
- `src-tauri/src/scan.rs`: algoritmo de varredura/agrupamento e testes.
- `src-tauri/src/optimize.rs`: adaptador nativo Windows e validação da letra da unidade.
- `src-tauri/src/health.rs`: diagnóstico de capacidade e confiabilidade por comandos Windows de leitura, com sensores opcionais.
- `src-tauri/src/lib.rs`: fronteira IPC; tarefas que leem disco ficam fora do thread principal.

## Decisões seguintes

| Prioridade | Capacidade | Critério de aceite |
| --- | --- | --- |
| P0 | Progresso por eventos e cancelamento | Interromper uma varredura sem deixar operação órfã; contadores atualizados |
| P0 | SQLite WAL + cache por file-id, tamanho, mtime | Nova varredura incremental comprovadamente evita rehash desnecessário |
| P0 | Guardas para hardlinks, sparse files e reparse points | Economia potencial não conta aliases como conteúdo livre |
| P1 | Inventário de volumes (WMI/Storage APIs) | Distinguir HDD, SSD NVMe, removível, rede e suportes não conhecidos |
| P1 | Diagnóstico SMART opcional e política | Sugerir análise antes de ação, com unidades sem suporte bloqueadas |
| P1 | Busca por glob e content-grep em arquivos texto | Pesquisa em streaming, com limite de bytes e opções de exclusão |
| P1 | TreeMap hierárquico com drill-down | Visualizar consumo inclusivo/exclusivo por pasta sem dupla contagem |
| P1 | Mapa de riscos com snapshot e diff | Detectar crescimento de espaço por pasta e por período |
| P2 | Duplicados semelhantes via fingerprints perceptuais | Separar visualmente proximidade de identidade exata; sem exclusão automática |
| P2 | Limpeza assistida/Quarentena com undo | Preview do plano, logs, política de retenção e restauração comprovada |
| P2 | Serviço nativo por SO | Adaptadores Linux/macOS independentes, com avaliação de FS e privilégios |

## Métricas de comparação contra Sparkling

- Velocidade: arquivos por segundo, MB lidos, cold/warm scan, CPU pico e RSS.
- Qualidade: falsos positivos de duplicidade, exclusão de hardlinks e contador de erros.
- UX: tempo até primeira evidência, custo de navegação, acessibilidade e robustez a pastas gigantes.
- Segurança: zero mutações inesperadas, zero acesso à rede, operação abortável e auditável.

Sem benchmark antes/depois, não afirmar superioridade numérica.

## Evidências externas para roadmap (pesquisa Exa, 08/10/2026)

- [Microsoft Learn: Optimize-Volume](https://learn.microsoft.com/en-us/powershell/module/storage/optimize-volume?view=windowsserver2025-ps): operações nativas dependem do tipo de volume/mídia. Evitar desfragmentação indiscriminada em SSD.
- [Microsoft Learn: Get-StorageReliabilityCounter](https://learn.microsoft.com/en-us/powershell/module/storage/get-storagereliabilitycounter?view=windowsserver2025-ps): temperatura, desgaste, erros e horas de uso dependem de suporte do dispositivo/driver; valores ausentes devem permanecer indisponíveis.
- [WinDirStat #340](https://github.com/windirstat/windirstat/issues/340) e [#108](https://github.com/windirstat/windirstat/issues/108): preocupações concretas com hardlinks e contabilidade de espaço físico.
- [Microsoft Q&A — hidden / unknown space](https://learn.microsoft.com/en-us/answers/questions/1688633/mismatch-of-used-disk-space-unknown-files-in-windi): demanda por explicação de diferenças entre alocação física e soma lógica; planejar diagnóstico de NTFS, VSS, metadados, permissões e arquivos especiais.
- [rsdirstat: cloud placeholders](https://github.com/rikshot/rsdirstat/commit/1faa0b3d1cc8b5dfb968ab598c127a8daf45898b): arquivos de nuvem sob demanda e reparse points exigem tratamento especial de tamanho alocado e recursão.

## Ciclo de segurança: autorização de otimização e hidratação de arquivos (08/10/2026)

- Uma análise bem-sucedida gera autorização **somente na memória do processo Rust**, associada à letra da unidade, com expiração de cinco minutos. A autorização é consumida antes de chamar o comando modificador; falha de análise ou nova análise a revoga. Isso impede que o frontend invoque a otimização diretamente sem diagnóstico prévio, mas não substitui confirmação humana e backups.
- A política conservadora de hash evita abrir arquivos marcados como offline, reparse ou recall-on-access no Windows. Metadados continuam visíveis na árvore. Condições de corrida entre metadados e abertura ainda existem; o produto não afirma proteção absoluta contra hidratação.
- Evidência de dor real: [WinDirStat #416](https://github.com/windirstat/windirstat/issues/416), downloads involuntários durante navegação de diretórios OneDrive.
- Fontes oficiais: [Optimize-Volume](https://learn.microsoft.com/en-us/powershell/module/storage/optimize-volume) (política por tipo de mídia) e [Get-StorageReliabilityCounter](https://learn.microsoft.com/en-us/powershell/module/storage/get-storagereliabilitycounter) (sensores dependentes de suporte do hardware).

### Testes ainda necessários em Windows

1. Rejeitar `execute=true` sem análise, após 5 minutos, em outra unidade ou depois de uma execução; testar falha e concorrência.
2. Verificar com Files On-Demand do OneDrive que pastas e placeholders aparecem no inventário sem download durante hash.
3. Testar troca/modificação de arquivo enquanto ocorre BLAKE3; relatório deve indicar resultado parcial.
4. Executar testes Rust, frontend e integração em volume descartável. CI atual está bloqueado e não fornece evidência de aprovação.
