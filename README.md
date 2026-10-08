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
git switch feat/storage-intelligence-mvp
npm install
npm run tauri dev
```

O código ainda está em uma branch de feature, que deverá ser revisada antes de entrar em `main`.

`npm run dev` sozinho mostra a interface no navegador, **mas não executa o scanner Rust**. Use `npm run tauri dev` para análise de arquivos e integração com o Windows.

### Funcionalidades presentes (v0.1)

1. Seleção da pasta via diálogo nativo (sem salvar o conteúdo na nuvem).
2. Varredura de tamanho lógico, total de arquivos, maiores diretórios, ranking de arquivos e distribuição por extensão.
3. Filtro por expressão regular no caminho/nome e tamanho mínimo em MB.
4. Identificação de duplicados **exatos**: primeiro agrupa por tamanho e depois confirma com hash completo BLAKE3, dentro do limite de leitura.
5. Visão de grupos duplicados, hash e caminhos para revisão manual — **sem botão de exclusão**.
6. Análise de fragmentação do volume no Windows e otimização explícita via PowerShell, com política do próprio Windows para HDD/SSD/tiered.
7. Mensagens visíveis para entradas inacessíveis, varreduras truncadas e análise de hash parcial.

**Importante:** tamanhos e economia potencial são valores lógicos; não equivalem necessariamente a blocos físicos livres (hardlinks, compressão, sparse files, deduplicação do filesystem). Não exclua arquivos somente pela semelhança de hash; verifique semântica, acesso e backup.

### Limites operacionais iniciais

- Até **250.000 arquivos** por varredura da interface (núcleo aceita até 1 milhão); o resultado marca explicitamente truncamento.
- Orçamento de até **8 GiB de conteúdo lido** para hash de candidatos duplicados, priorizando grupos de arquivos maiores. Fora desse orçamento, o relatório avisa que pode haver mais cópias.
- Exibe até 300 maiores arquivos, 300 diretórios, 300 grupos duplicados e 500 correspondências de busca.
- Leitura direta local, sem indexação persistente nesta versão; cada nova busca executa nova varredura.
- Não segue symlinks. Erros de acesso são contabilizados.
- O scanner é assíncrono do ponto de vista da UI, mas ainda não implementa cancelamento, progresso incremental ou snapshot SQLite.
- O módulo de otimização é Windows-only. Não força `-Defrag` em SSD, não eleva privilégios e não agenda tarefas automaticamente.
- Funcionalidade de pré-visualização do impacto, métricas SMART, snapshot SQLite incremental, content-grep e tratamentos de hardlinks estão planejados.

## Comandos de qualidade

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
```

Os testes Rust incluem confirmação de hash para duplicados, Regex inválida, limite de varredura e validação da letra da unidade. CI executa build frontend e testes nativos no Windows; formatação Rust deve ser verificada localmente antes de merge.

Leia [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para módulos, prioridades, política de disco e roadmap.
