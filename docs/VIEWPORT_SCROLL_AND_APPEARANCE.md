# Thorn Intelligence — Correção de viewport, scroll e personalização

## Problema reproduzido visualmente

Na captura enviada pelo usuário, o Thorn apresenta uma janela arredondada **dentro** do viewport Tauri, com borda externa duplicada, rolagem global à direita (movimenta até o chrome visual), hero grande que parece ocupar todo o espaço e ausência de controles para personalizar o vidro. Esses sintomas correspondem ao CSS observado:

- \`styles.css\`: \`.app-shell{min-height:100vh}\`, \`.sidebar{height:100vh;position:sticky}\`.
- \`glass-studio.css\`: \`width:calc(100% - 36px)\`, \`margin:18px auto\`, \`min-height:calc(100vh - 36px)\`, \`.sidebar{height:calc(100vh - 36px);min-height:650px}\`.
- \`App.tsx\`: todo o conteúdo (header + sidebar + painel principal) era rolado junto com a página, e só existia o toggle **Contraste**.
- Configuração Tauri já usa \`transparent:true\`, \`decorations:false\`, \`shadow:false\` no primeiro paint e depois restaura a decoração Win32. Nenhuma mudança no backend era necessária para solucionar a rolagem da UI.

## Correção estrutural

1. \`html\`, \`body\`, \`#root\` usam a altura exata do WebView, \`overflow:hidden\`, sem rolagem global. \`.glass-studio\` ocupa 100% da área **cliente da janela nativa**, com \`margin:0\`, \`border:0\`, \`border-radius:0\`. O usuário pode optar por uma moldura discreta, mas ela **não aumenta** a altura do documento.
2. O \`.main-shell\` é flex-column com \`min-height:0\`, \`overflow:hidden\`. O \`.topbar\` mantém sua altura; o \`main.content\` é o único scrollport externo (\`flex:1;min-height:0;overflow-y:auto;overscroll-behavior:contain\`). A sidebar mantém rolagem própria **apenas se a navegação não couber**. Isso elimina a barra de scroll externa à direita do aplicativo e a impressão de que o chrome inteiro pula ao rolar.
3. Scrollbar vertical fina, adaptada ao grafite e de largura estável; \`IndexedTree\` preserva seu scroll independente para linhas virtualizadas. Ao trocar de seção, o conteúdo retorna ao topo, sem scroll suave que gere tremores.
4. O drawer mobile mantém largura máxima, \`position:fixed\`, overlay para fechar ao tocar fora e conteúdo principal no seu scrollport.

## Personalização implementada

Menu **Personalizar** acessível por teclado, no cabeçalho:

- **Cor de destaque:** Lavanda, Menta, Coral ou Azul. Afeta botão principal, seleção de navegação e destaques; não substitui cores semânticas de erro/atenção.
- **Densidade:** Confortável/Compacta; altera espaçamentos e dimensões, sem zoom artificial nem sobreposição de controles.
- **Moldura interna:** \`Sem borda extra\` (padrão) / \`Moldura discreta\`. A borda discreta fica **dentro** do viewport e não altera a política de scroll.
- **Opacidade dos painéis:** slider de 20% a 85%, afetando cartões e superfícies de vidro; inativo no modo sólido.
- **Painéis sólidos:** mantém alternância de alto contraste existente.
- **Restaurar aparência padrão** e persistência local (localStorage). Todas as alterações são aplicadas sem reiniciar, sem gravar dados sensíveis e sem tocar em arquivos analisados.

## Testes recomendados

- Abrir no Windows 10/11 em 1010×680, 1420×900 e tela maximizada. **Não deve existir barra de scroll global**; apenas o conteúdo interno deve rolar. Topbar/sidebar permanecem posicionados.
- Alternar seções Visão geral, Explorador, Duplicados, Comparar, Quarentena, Busca e Otimização. Conteúdo inicia do topo, mesmo após navegar no fim da seção anterior.
- Exercitar árvore SQLite grande: scroll dentro da árvore não deve transferir momentum para o viewport; não cortar elementos interativos.
- Alterar todas as preferências e reiniciar: persistem as escolhas, sem deformar posicionamento. Testar restaurar padrão.
- Usar a opção de moldura discreta e verificar que ela não recria barras de scroll do documento.
- Acessibilidade: teclado/Tab e Escape no menu Personalizar, prefers-reduced-motion, contraste e foco visível.
- Benchmark WebView2: compare GPU/FPS com opacidade 20%/85% e modo sólido. CSS blur não prova transparência DWM.
- Executar \`npm run build\` e \`cargo test --manifest-path src-tauri/Cargo.toml\`, além de teste visual nativo. Se GitHub Actions falhar antes dos steps, não tratar como aprovação.

## Fontes pesquisadas com Exa

- [Tauri issue #8308, transparência da janela Tauri 2](https://github.com/tauri-apps/tauri/issues/8308)
- [WebView2Feedback #4131, renderização da scrollbar nativa](https://github.com/MicrosoftEdge/WebView2Feedback/issues/4131)
- [Discussão de overscroll WebView2](https://github.com/martinkoutecky/tine/issues/177)
- [Tauri #7328, transparência no Windows 11](https://github.com/tauri-apps/tauri/issues/7328)

As causas foram derivadas da **captura do usuário e do código CSS do Thorn**. As fontes externas explicam comportamentos possíveis da plataforma, mas não substituem a validação na máquina Windows real.
