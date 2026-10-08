# Segurança: índice SQLite e quarentena reversível

## Indexação incremental (somente metadados)

O SQLite fica em AppLocalData, usa rusqlite 0.40.2 com SQLite incorporado, WAL, synchronous=FULL e timeout de escrita. A enumeração deposita metadados numa tabela TEMP exclusiva da conexão (SQLite pode usar arquivo temporário local), sem manter uma transação de escrita no banco principal durante a caminhada. Ao final, uma transação atômica publica a geração e remove entradas ausentes; a transação ainda pode bloquear outros escritores durante essa fase final. A cada atualização, caminho/tamanho/mtime-ns/atributos são comparados para classificar novos/alterados/inalterados. Cancelamento, entradas inacessíveis e limite de arquivos descartam o estágio temporário ao fechar a conexão; o snapshot anterior permanece consultável. O custo adicional de staging e o tempo de COMMIT para milhões de arquivos ainda exigem benchmarks.

A pesquisa em cache consulta a última geração completa sem abrir arquivos de usuário. Até 500 resultados são retornados, e a data do snapshot é exibida para indicar possível desatualização. Ainda não é USN Journal, monitoramento em tempo real nem cache BLAKE3; NÃO reutilizar hashes com base apenas em mtime/tamanho. Diretórios reparse/junction/cloud são excluídos, com contagem explícita.

## Quarentena (Windows) — mutação somente após confirmação

O backend rejeita arquivos não regulares, links simbólicos, reparse points, arquivos offline/recall/system, hardlinks, diretórios protegidos, AppData (exceto TEMP local) e locais OneDrive conhecidos. A pré-visualização conserva identificador UUID, tamanho, mtime e identidade Win32, com TTL 5 min e uso único. Ao mover, verifica novamente metadados e exige a frase MOVER PARA QUARENTENA; não aceita um caminho arbitrário no comando modificador.

O destino é AppLocalData/quarantine/UUID, com manifesto SQLite registrado como prepared ANTES de mover. MoveFileExW é chamado sem COPY_ALLOWED, sem REPLACE_EXISTING e sem operação adiada: outro volume ou destino ocupado causam erro, sem cópia+deleção. Se o processo cair após o movimento, um registro prepared e arquivo existente permanece detectável para restauração.

Restaurar exige a frase RESTAURAR, manifesta estado prepared/quarantined, confere identidade/tamanho na quarentena, exige pasta original válida e impede sobrescrever o caminho original. Caso o caminho tenha sido reutilizado, a restauração é recusada. Não existe purga permanente, retenção automática nem exclusão em lote.

ATENÇÃO: mover arquivos ao mesmo volume NÃO libera espaço físico. A quarentena não substitui backup, não garante recuperação de todo estado do Windows e não elimina a janela de race condition entre inspeção por caminho e MoveFileExW. Evitar uso em diretórios controlados por outro processo adversarial até implantar movimentação por handle.

## Testes e release gates

Executar no Windows, com Node 22, Rust stable, Visual Studio Build Tools e SDK:
- npm install e npm run build
- cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
- cargo test --manifest-path src-tauri/Cargo.toml
- cargo check --manifest-path src-tauri/Cargo.toml
- npm run tauri dev

Testes incluídos: criação de snapshot, refresh sem mudanças, mudanças/remoções, cancelamento que preserva índice, escrita concorrente durante enumeração com staging temporário, confirmação e UUID inválidos, e no Windows round-trip de quarentena/restauração sem sobrescrever colisões. Validar ainda OneDrive Files On-Demand, arquivos sparse, ACLs, hardlinks, NTFS/ReFS, unidades C: versus D:, crash durante WAL/manifesto, e concorrência com renomeações.

Não considerar homologado se CI não iniciou os jobs.

## Evidências via Exa

- SQLite WAL: https://sqlite.org/wal.html
- SQLite WAL-reset corrections in 2026: https://sqlite.org/news.html
- rusqlite bundled 0.40.2 (SQLite 3.53.2): https://docs.rs/crate/rusqlite/latest
- Microsoft MoveFileExW flags: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
- Microsoft reparse points: https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points
- Rust stable limitations on Windows MetadataExt: https://doc.rust-lang.org/std/os/windows/fs/trait.MetadataExt.html
