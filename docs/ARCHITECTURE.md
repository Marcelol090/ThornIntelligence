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
 │         ├── BLAKE3 de conteúdo (até 8 GiB lidos)
 │         └── identidade do arquivo (same-file) para descartar hardlinks
 └── optimize_volume({drive,execute})
      └── Windows PowerShell > Optimize-Volume
           ├── execute=false: -Analyze
           └── execute=true: modo automático do Windows
```

O frontend recebe relatórios serializados (camelCase) e apresenta **apenas informações medidas**. Dados não são enviados para serviços externos.

### Segurança operacional

- Não implementar exclusão silenciosa, deduplicação via hardlink ou limpeza de cache sem revisão e opt-in.
- Exigir confirmação explícita para toda operação modificadora. Modo análise nunca otimiza.
- Usar `Command::new` com argumentos fixos, aceitando apenas uma letra ASCII de unidade.
- Deixar o sistema operacional identificar HDD/SSD/tiers; não usar `-Defrag` indiscriminadamente.
- Respeitar privilégios do usuário e mostrar falhas de permissão.
- Não seguir junctions/symlinks durante a varredura nem atravessar outros destinos intencionalmente por links.
- Não afirmar economia física: APFS/NTFS sparse/reparse/hardlinks/compressão alteram cálculo real.
- Descartar hardlink aliases na estimativa com verificação de identidade via `same-file`; impor limite de descritores e marcar grupos não verificados como parciais.
- Não declarar duplicação por nome/tamanho/partial hash apenas: o candidato exige digest integral; hashes são uma evidência forte, não verificação byte a byte para situações adversariais.
- Sem comandos genéricos digitados pelo usuário, sem execução remota.

### Arquivos de origem

- `src/App.tsx`: navegação, estados, busca, tabelas, ações explícitas, comunicação Tauri.
- `src/styles.css`: tokens visuais dark/glass, backdrop-filter, breakpoints, foco visível e reduced motion.
- `src-tauri/src/scan.rs`: algoritmo de varredura/agrupamento e testes.
- `src-tauri/src/optimize.rs`: adaptador nativo Windows e validação da letra da unidade.
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
