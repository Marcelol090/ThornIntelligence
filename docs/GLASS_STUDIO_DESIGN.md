# Thorn Intelligence — Glass Studio (direção visual)

## Referência fornecida pelo usuário

A referência de Channel Analytics representa um produto desktop premium: moldura flutuante grafite, sidebar compacta com ícones, superfícies foscas semitransparentes, tons quentes, acentos lavanda/menta/coral, densidade moderada e contraste tipográfico claro. É uma referência de linguagem visual, não de domínio funcional: o Thorn continua sendo um analisador de armazenamento.

## Adaptação no Thorn

- Shell: moldura escura com raio generoso, borda e sombras moderadas. No Windows, quando o efeito nativo está disponível, fundos com alpha permitem composição do DWM; sem ele há fallback grafite.
- Rail: ícones com opção de expandir os rótulos; cada botão conserva aria-label, title, foco e indicador de página atual. Na largura estreita, sidebar permanece uma gaveta.
- Superfícies: cartões, header, tabela, explorador, árvore e formulários compartilham paleta de vidro grafite com bordas suaves. Cores semânticas de erro e atenção não se confundem com decoração.
- Panorama: o gráfico do hero só aparece após varredura real, usando report.fileTypes e bytes lógicos. Barras relativas ao maior tipo, sem números inventados nem promessa de espaço recuperável. O onboarding sem relatório tem ilustração decorativa oculta para tecnologias assistivas.
- Acessibilidade: botão Contraste alterna painéis sólidos e salva a escolha só localmente. prefers-reduced-motion, prefers-contrast, forced-colors e foco visível estão cobertos.
- Windows: preservados decorations:false, shadow:false e transparent:true; decoração restabelecida antes do efeito. Acrylic é solicitado primeiro, com Mica como fallback de falha de chamada, nunca ambos simultaneamente. Indicador informa pedido encaminhado, não prova de composição DWM.

## Arquivos

- src/styles.css — estilos existentes.
- src/glass-studio.css — tokens e refinamento visual. Importado depois do CSS legado, sem reescrever todos os componentes.
- src/App.tsx — rail recolhível, modo Contraste, status do efeito e gráficos baseados em dados reais.
- src/main.tsx — importa o novo CSS.
- Nenhuma alteração de Rust, hashing, NTFS, SQLite, quarentena nem execução de ações sobre arquivos.

## Critérios de aceite

1. Windows 10/11: verificar primeiro paint, resize, minimizar/restaurar e transparência do SO desativada. Promise resolvida do Tauri não equivale a efeito composto.
2. Teclado e leitor de tela: navegar rail compacto, expandido e gaveta; trocar seções sem perder trabalho.
3. Layout: 1010x680 (mínimo Tauri), 1420x900, 1920x1080 e viewport web estreita; evitar sobreposição com controles de segurança e páginas de análise.
4. Gráfico: nenhum dado sintético apresentado como inventário. Em ausência de relatório, somente imagem decorativa.
5. WebView2: inspecionar GPU/FPS com muitos efeitos de blur; modo Contraste deve permitir superfícies sólidas.
6. Validar npm run build, cargo test --manifest-path src-tauri/Cargo.toml e smoke-test da aplicação nativa. Não declarar sucesso com CI que falhou antes dos steps.

## Pesquisa Exa e fontes oficiais

- https://github.com/tauri-apps/tauri/issues/12804 — transparência e backdrop-filter
- https://github.com/tauri-apps/window-vibrancy — materiais nativos
- https://github.com/MicrosoftEdge/WebView2Feedback/issues/4945 — alpha zero e filtros CSS
- https://github.com/MicrosoftEdge/WebView2Feedback/issues/5409 — regressão Mica no WebView2
- https://github.com/tauri-apps/tauri/issues/10318 — primeiro paint transparente

A fotografia, marca e métricas de redes sociais na referência não são reproduzidas. A adaptação usa a composição e o material visual aplicados a dados de armazenamento reais.
