import { useState, type ComponentType, type FormEvent, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import {
  Activity, AlertCircle, ArrowRight, ArrowUpRight,
  BarChart3, Check, CheckCircle2, ChevronRight, CircleHelp, Clipboard,
  Database, Disc3, File, FileSearch, Fingerprint, Folder, FolderOpen,
  Gauge, HardDrive, Info, Layers3, LoaderCircle, LockKeyhole, Menu,
  Search, ShieldCheck, SlidersHorizontal, Sparkles, WandSparkles, X,
} from 'lucide-react';
import type { DiskHealth, DuplicateGroup, FileResult, OptimizationResult, ScanReport, ScanRequest, Section } from './types';
import { bytes, duration, number, truncatePath } from './lib/format';

type IconType = ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
const links: { id: Section; label: string; icon: IconType }[] = [
  { id: 'overview', label: 'Visão geral', icon: BarChart3 },
  { id: 'explorer', label: 'Explorador', icon: FolderOpen },
  { id: 'duplicates', label: 'Duplicados', icon: Layers3 },
  { id: 'search', label: 'Busca inteligente', icon: FileSearch },
  { id: 'optimize', label: 'Otimização', icon: Gauge },
];

function Metric({ label, value, helper, icon: Icon, tone = 'blue' }: {
  label: string; value: string; helper: string; icon: IconType; tone?: string;
}) {
  return (
    <div className="metric glass">
      <div className="metric-top"><span className={'metric-icon tone-' + tone}><Icon size={19}/></span><ArrowUpRight size={16} className="muted-icon"/></div>
      <div className="metric-value">{value}</div>
      <div className="metric-name">{label}</div>
      <div className="metric-helper">{helper}</div>
    </div>
  );
}

function SectionHeading({ kicker, title, description, right }: {
  kicker: string; title: string; description?: string; right?: ReactNode;
}) {
  return <div className="section-heading">
    <div><p className="eyebrow">{kicker}</p><h2>{title}</h2>{description && <p className="section-description">{description}</p>}</div>
    {right}
  </div>;
}

function Tag({ children, tone = 'neutral' }: { children: ReactNode; tone?: string }) {
  return <span className={'tag tag-' + tone}>{children}</span>;
}

function FileRows({ files, copy }: { files: FileResult[]; copy: (path: string) => void }) {
  if (!files.length) return <div className="empty-list">Nenhum arquivo encontrado nesta visualização.</div>;
  return <div className="file-table">
    <div className="file-table-head"><span>ARQUIVO</span><span>TIPO</span><span>TAMANHO</span><span></span></div>
    {files.map((file) => <div className="file-row" key={file.path}>
      <span className="file-identity"><span className="file-icon"><File size={18}/></span><span className="file-text"><strong title={file.name}>{file.name}</strong><small title={file.path}>{truncatePath(file.path, 64)}</small></span></span>
      <span className="type-cell">{file.extension}</span><strong className="size-cell">{bytes(file.sizeBytes)}</strong>
      <button className="icon-button" type="button" title="Copiar caminho" aria-label={'Copiar caminho de ' + file.name} onClick={() => copy(file.path)}><Clipboard size={16}/></button>
    </div>)}
  </div>;
}

function DuplicateCard({ group, copy }: { group: DuplicateGroup; copy: (path: string) => void }) {
  const [expanded, setExpanded] = useState(false);
  return <div className="duplicate-card glass">
    <div className="duplicate-top">
      <div className="duplicate-avatar"><Fingerprint size={22}/></div>
      <div className="duplicate-summary"><strong>{group.copies.length} cópias idênticas</strong><span>Hash BLAKE3 · {group.hash.slice(0, 16)}…</span></div>
      <div className="duplicate-savings"><small>Economia potencial</small><strong>{bytes(group.potentialSavingsBytes)}</strong></div>
      <button className="subtle-button" type="button" onClick={() => setExpanded(!expanded)} aria-expanded={expanded}>{expanded ? 'Ocultar' : 'Ver arquivos'} <ChevronRight className={expanded ? 'rotated' : ''} size={15}/></button>
    </div>
    {expanded && <div className="duplicate-files">{group.copies.map((path) => <div key={path}><span title={path}>{truncatePath(path, 100)}</span><button className="icon-button" aria-label="Copiar caminho" onClick={() => copy(path)} title="Copiar caminho"><Clipboard size={15}/></button></div>)}</div>}
  </div>;
}

export default function App() {
  const [section, setSection] = useState<Section>('overview');
  const [report, setReport] = useState<ScanReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [toast, setToast] = useState('');
  const [regex, setRegex] = useState('');
  const [minMb, setMinMb] = useState('0');
  const [drive, setDrive] = useState('C');
  const [confirmOptimize, setConfirmOptimize] = useState(false);
  const [optimization, setOptimization] = useState<OptimizationResult | null>(null);
  const [optimizing, setOptimizing] = useState(false);
  const [diskHealth, setDiskHealth] = useState<DiskHealth | null>(null);
  const [healthBusy, setHealthBusy] = useState(false);
  const [mobileMenu, setMobileMenu] = useState(false);

  async function copy(value: string) {
    try { await navigator.clipboard.writeText(value); setToast('Caminho copiado.'); }
    catch { setToast('Não foi possível copiar o caminho.'); }
  }

  async function scanFolder(root?: string, pattern = regex, min = minMb) {
    setError('');
    let path = root;
    if (!path) {
      try {
        const selected = await open({ directory: true, multiple: false, title: 'Escolher diretório para análise' });
        if (!selected || typeof selected !== 'string') return;
        path = selected;
      } catch (err) {
        setError('Seleção de pasta indisponível. Abra o aplicativo com npm run tauri dev. Detalhes: ' + String(err));
        return;
      }
    }
    setBusy(true);
    try {
      const parsed = Number(min);
      if (!Number.isFinite(parsed) || parsed < 0) throw new Error('Tamanho mínimo inválido.');
      const request: ScanRequest = { root: path, regex: pattern.trim() || null, minSizeBytes: Math.floor(parsed * 1024 * 1024), maxFiles: 250_000 };
      const result = await invoke<ScanReport>('scan_path', { request });
      setReport(result);
      setToast('Análise concluída em ' + duration(result.elapsedMs) + '.');
    } catch (err) { setError(String(err)); }
    finally { setBusy(false); }
  }

  async function checkDiskHealth() {
    setError('');
    setDiskHealth(null);
    setHealthBusy(true);
    try {
      const health = await invoke<DiskHealth>('disk_health', { drive });
      setDiskHealth(health);
    } catch (err) {
      setError('O diagnóstico não pôde ser concluído: ' + String(err));
    } finally {
      setHealthBusy(false);
    }
  }

  async function runOptimization(execute: boolean) {
    setError('');
    if (execute && !confirmOptimize) {
      setError('Confirme que entende o impacto antes de iniciar a otimização.');
      return;
    }
    if (execute && (!optimization || optimization.executed || optimization.drive !== drive.trim().replace(':', '').toUpperCase())) {
      setError('Analise esta unidade com sucesso antes de iniciar a otimização.');
      return;
    }
    setOptimization(null);
    setOptimizing(true);
    try {
      const result = await invoke<OptimizationResult>('optimize_volume', { drive, execute });
      setOptimization(result);
      setConfirmOptimize(false);
    } catch (err) { setError(String(err)); }
    finally { setOptimizing(false); }
  }

  function selectSection(value: Section) {
    setSection(value);
    setError('');
    setMobileMenu(false);
  }

  const scanned = report !== null;
  const roots = report?.root ?? '';
  const scopeName = roots ? (roots.split(/[\\/]/).filter(Boolean).pop() ?? roots) : 'Nenhuma pasta selecionada';

  return <div className="app-shell">
    <aside className={'sidebar glass ' + (mobileMenu ? 'sidebar-open' : '')}>
      <div className="brand">
        <div className="brand-symbol"><Disc3 size={23} strokeWidth={2.1}/></div>
        <div className="brand-words"><strong>THORN<span>INTELLIGENCE</span></strong><small>STORAGE CONSOLE</small></div>
        <button className="mobile-close icon-button" onClick={() => setMobileMenu(false)} aria-label="Fechar menu"><X size={19}/></button>
      </div>
      <div className="side-group-label">WORKSPACE</div>
      <nav className="nav-menu" aria-label="Navegação principal">
        {links.map(({ id, icon: Icon, label }) => <button type="button" key={id} className={'nav-link ' + (section === id ? 'selected' : '')} onClick={() => selectSection(id)} aria-current={section === id ? 'page' : undefined}>
          <Icon size={19}/><span>{label}</span>{section === id && <span className="nav-indicator"/>}
        </button>)}
      </nav>
      <div className="sidebar-spacer"/>
      <div className="side-device glass-inset"><div className="device-avatar"><HardDrive size={19}/></div><div><strong>Processamento local</strong><small>Nenhum upload de arquivos</small></div><CheckCircle2 size={17} className="success-icon"/></div>
      <div className="side-help"><ShieldCheck size={16}/> Leitura e diagnóstico seguros</div>
      <div className="side-footer"><span className="live-dot"/> ENGINE V0.1 <span className="footer-version">BETA</span></div>
    </aside>

    <div className="main-shell">
      <header className="topbar">
        <div className="crumb"><button className="menu-button icon-button" aria-label="Abrir menu" onClick={() => setMobileMenu(true)}><Menu size={20}/></button><span>Workspace</span><ChevronRight size={14}/><strong>{links.find((link) => link.id === section)?.label}</strong></div>
        <div className="top-actions"><span className="local-pill"><span className="live-dot"/> LOCAL-FIRST</span><button className="top-help" onClick={() => selectSection('optimize')} title="Conheça os controles de segurança" aria-label="Segurança"><CircleHelp size={19}/></button><div className="user-avatar"><Disc3 size={17}/></div></div>
      </header>
      <main id="main-content" className="content">
        <div className="hero-heading">
          <div><div className="hero-kicker"><Sparkles size={14}/> ARMAZENAMENTO SOB CONTROLE</div><h1>{links.find((link) => link.id === section)?.label}<span className="heading-period">.</span></h1><p>Descubra o que ocupa espaço, identifique desperdícios e tome decisões com segurança.</p></div>
          <button className="primary-button" disabled={busy} onClick={() => void scanFolder()}>{busy ? <LoaderCircle className="spin" size={18}/> : <FolderOpen size={18}/>} {busy ? 'Analisando…' : 'Analisar pasta'} <ArrowRight size={16}/></button>
        </div>

        <div className="scope-strip glass"><div className="scope-icon"><Folder size={19}/></div><div className="scope-details"><small>ESCOPO ATUAL</small><strong title={roots}>{scopeName}</strong></div><span className="scope-full" title={roots}>{scanned ? truncatePath(roots, 56) : 'Escolha uma pasta ou unidade para iniciar'}</span><Tag tone={scanned ? 'green' : 'neutral'}>{scanned ? 'ANALISADO' : 'AGUARDANDO'}</Tag></div>

        {error && <div role="alert" className="alert error-alert"><AlertCircle size={19}/><span>{error}</span><button aria-label="Fechar aviso" className="icon-button" onClick={() => setError('')}><X size={16}/></button></div>}
        {toast && <div role="status" className="alert toast-alert"><Check size={17}/><span>{toast}</span><button aria-label="Fechar mensagem" className="icon-button" onClick={() => setToast('')}><X size={16}/></button></div>}
        {report && (report.truncated || report.errors > 0 || !report.duplicateAnalysisComplete || report.hardlinkAliases > 0) && <div className="alert warning-alert"><Info size={18}/><span>{report.truncated ? 'Limite de 250.000 arquivos atingido; o relatório é parcial. ' : ''}{report.errors > 0 ? number(report.errors) + ' entradas não puderam ser processadas. ' : ''}{!report.duplicateAnalysisComplete ? 'Limite de 8 GB de leitura por hash atingido; pode haver mais duplicados. ' : ''}{report.hardlinkAliases > 0 ? number(report.hardlinkAliases) + ' links físicos compartilhados foram excluídos das estimativas. ' : ''}As estimativas não equivalem a espaço liberado.</span></div>}

        {!scanned && section !== 'optimize' && <div className="onboarding glass">
          <div className="onboarding-content"><Tag tone="blue"><Sparkles size={13}/> INTELLIGENT STORAGE</Tag><h2>Encontre espaço que você nem sabia que tinha.</h2><p>Mapeie arquivos, compare tamanhos, descubra duplicados reais com BLAKE3 e pesquise nomes ou caminhos usando expressões regulares. Tudo acontece no seu computador.</p><button className="primary-button" disabled={busy} onClick={() => void scanFolder()}><FolderOpen size={18}/> Escolher pasta <ArrowRight size={17}/></button></div>
          <div className="onboarding-visual"><div className="orb orb-one"/><div className="orb orb-two"/><div className="preview-card preview-main"><div className="preview-dotline"><i/><i/><i/></div><div className="preview-chart"><div/><div/><div/><div/><div/><div/><div/></div><div className="preview-baseline"/></div><div className="preview-card preview-small"><Fingerprint size={21}/><span>BLAKE3</span><CheckCircle2 size={17}/></div></div>
        </div>}

        {section === 'overview' && report && <>
          <div className="metrics-grid">
            <Metric label="Volume analisado" value={bytes(report.logicalBytes)} helper="Tamanho lógico dos arquivos" icon={Database}/>
            <Metric label="Arquivos indexados" value={number(report.filesScanned)} helper={number(report.directoriesScanned) + ' diretórios percorridos'} icon={FileSearch} tone="violet"/>
            <Metric label="Espaço duplicado" value={bytes(report.potentialSavingsBytes)} helper="Estimativa, sem exclusões" icon={Fingerprint} tone="mint"/>
            <Metric label="Grupos duplicados" value={number(report.duplicates.length)} helper={bytes(report.hashBytesRead) + ' lidos por hash'} icon={Layers3} tone="pink"/>
          </div>
          <div className="dashboard-grid">
            <section className="panel glass">
              <SectionHeading kicker="MAPA DE CONSUMO" title="Maiores diretórios" right={<button className="text-button" onClick={() => selectSection('explorer')}>Ver todos <ArrowRight size={16}/></button>}/>
              <div className="directory-list">{report.topDirectories.slice(0, 6).map((item, index) => <div className="directory-item" key={item.path}><div className="directory-row"><div className="directory-label"><span className="directory-icon"><Folder size={17}/></span><span title={item.path}>{item.name}</span></div><strong>{bytes(item.sizeBytes)}</strong></div><div className="bar-track"><div className={'bar-fill color-' + index % 4} style={{ width: (report.logicalBytes > 0 ? Math.max(1, item.sizeBytes / report.logicalBytes * 100) : 0) + '%' }}/></div></div>)}</div>
              {report.topDirectories.length === 0 && <div className="empty-list">Nenhum subdiretório encontrado.</div>}
              <p className="panel-note"><Info size={14}/> Pastas pai e filhas compartilham bytes; não some as barras.</p>
            </section>
            <section className="panel glass">
              <SectionHeading kicker="DISTRIBUIÇÃO" title="Por tipo de arquivo"/>
              <div className="distribution-bar">{report.fileTypes.slice(0, 7).map((item, index) => <div key={item.extension} title={item.extension + ': ' + bytes(item.sizeBytes)} className={'distribution-piece color-' + index % 7} style={{ width: (report.logicalBytes ? item.sizeBytes / report.logicalBytes * 100 : 0) + '%' }}/>)}</div>
              <div className="type-list">{report.fileTypes.slice(0, 6).map((item, index) => <div className="type-row" key={item.extension}><span><i className={'type-dot color-' + index % 7}/>{item.extension}</span><strong>{bytes(item.sizeBytes)}</strong><small>{report.logicalBytes ? (item.sizeBytes / report.logicalBytes * 100).toFixed(1) : 0}%</small></div>)}</div>
              <p className="panel-note">Percentuais calculados sobre o escopo selecionado.</p>
            </section>
          </div>
          <section className="panel glass wide-panel"><SectionHeading kicker="OPORTUNIDADES" title="Arquivos que mais ocupam espaço" right={<button className="text-button" onClick={() => selectSection('explorer')}>Explorar arquivos <ArrowRight size={16}/></button>}/><FileRows files={report.topFiles.slice(0, 7)} copy={copy}/></section>
        </>}

        {section === 'explorer' && report && <section className="panel glass full-panel"><SectionHeading kicker="RANKING POR TAMANHO" title="Arquivos grandes" description="Os 300 maiores arquivos encontrados na varredura; exibidos por tamanho lógico." right={<Tag tone="blue">{report.topFiles.length} resultados</Tag>}/><FileRows files={report.topFiles} copy={copy}/></section>}

        {section === 'duplicates' && report && <div className="stack-gap">
          <div className="insight-banner glass"><div className="insight-icon"><Fingerprint size={25}/></div><div><small>DUPLICAÇÃO VERIFICADA</small><strong>{bytes(report.potentialSavingsBytes)} de economia potencial</strong><p>Apenas cópias independentes, com mesmo tamanho e hash BLAKE3 idêntico, aparecem abaixo. Hardlinks foram excluídos das economias estimadas. Nenhum arquivo é removido.</p></div><Tag tone={report.duplicateAnalysisComplete ? 'green' : 'amber'}>{report.duplicateAnalysisComplete ? 'HASH COMPLETO*' : 'HASH PARCIAL'}</Tag></div>
          <SectionHeading kicker="INSPEÇÃO MANUAL" title="Grupos idênticos" description="Copie os caminhos e revise antes de qualquer intervenção."/>
          {report.duplicates.length ? report.duplicates.map((group) => <DuplicateCard group={group} key={group.hash} copy={copy}/>) : <div className="empty-list standalone">Nenhum grupo duplicado confirmado dentro do orçamento de hash.</div>}
          <p className="subnote">* Completo dentro do escopo analisado, sujeito a erros de leitura, limite de arquivos e links físicos.</p>
        </div>}

        {section === 'search' && report && <div className="stack-gap">
          <section className="panel glass search-panel">
            <SectionHeading kicker="BUSCA AVANÇADA" title="Pesquisa por Regex + tamanho" description="Filtre todo o escopo escolhido pelo nome/caminho, não apenas os arquivos visíveis."/>
            <form className="search-form" onSubmit={(event: FormEvent) => { event.preventDefault(); void scanFolder(report.root, regex, minMb); }}>
              <label className="field"><span>Expressão regular (nome ou caminho)</span><div className="input-wrap"><Search size={19}/><input type="text" placeholder="Ex.: \\.(iso|zip|mp4)$" value={regex} onChange={(e) => setRegex(e.target.value)} spellCheck={false}/></div></label>
              <label className="field min-field"><span>Tamanho mínimo (MB)</span><div className="input-wrap"><SlidersHorizontal size={18}/><input type="number" min="0" step="1" value={minMb} onChange={(e) => setMinMb(e.target.value)}/></div></label>
              <button className="primary-button" type="submit" disabled={busy}>{busy ? <LoaderCircle className="spin" size={17}/> : <Search size={17}/>} Buscar</button>
            </form>
            <p className="panel-note"><LockKeyhole size={14}/> Pesquisa no sistema de arquivos local, sem indexação em nuvem. Regex usa a sintaxe do Rust regex.</p>
          </section>
          <section className="panel glass"><SectionHeading kicker="RESULTADOS DE PESQUISA" title={number(report.totalMatches) + ' arquivos encontrados'} description="Exibindo até 500 resultados, em ordem decrescente de tamanho."/><FileRows files={report.matches} copy={copy}/></section>
        </div>}

        {section === 'optimize' && <div className="stack-gap">
          <div className="optimizer-header glass"><div className="optimization-visual"><span className="optimization-glow"/><HardDrive size={54}/></div><div><Tag tone="mint"><ShieldCheck size={13}/> POLÍTICA DE SEGURANÇA</Tag><h2>Otimização por tipo de mídia.</h2><p>O Windows escolhe a ação adequada ao volume. HDD pode receber desfragmentação; SSD compatível recebe ReTRIM. Nunca forçamos a desfragmentação de SSD.</p></div></div>
          <section className="panel glass health-panel">
            <SectionHeading kicker="DIAGNÓSTICO SOMENTE LEITURA" title="Saúde e capacidade do disco" description="Consulta dados reais do Windows Storage para a unidade selecionada. Nenhum reparo ou teste de estresse é iniciado." right={<button type="button" className="outline-button" disabled={healthBusy} onClick={() => void checkDiskHealth()}>{healthBusy ? <LoaderCircle size={16} className="spin"/> : <Activity size={16}/>} Consultar saúde</button>}/>
            {diskHealth ? <>
              <div className="health-summary"><HardDrive size={19}/><strong>{diskHealth.model || 'Disco sem identificação'}</strong><Tag tone={diskHealth.healthStatus === 'Healthy' ? 'green' : 'amber'}>{diskHealth.healthStatus || 'Estado desconhecido'}</Tag><span>{diskHealth.busType || 'Barramento desconhecido'}</span></div>
              <div className="health-grid">
                <div><small>Capacidade</small><strong>{diskHealth.sizeBytes == null ? 'Indisponível' : bytes(diskHealth.sizeBytes)}</strong></div>
                <div><small>Espaço livre</small><strong>{diskHealth.freeBytes == null ? 'Indisponível' : bytes(diskHealth.freeBytes)}</strong></div>
                <div><small>Temperatura</small><strong>{diskHealth.temperatureC == null ? 'Indisponível' : diskHealth.temperatureC + ' °C'}</strong></div>
                <div><small>Desgaste reportado</small><strong>{diskHealth.wearPercent == null ? 'Indisponível' : diskHealth.wearPercent + '%'}</strong></div>
                <div><small>Erros de leitura não corrigidos</small><strong>{diskHealth.readErrorsUncorrected == null ? 'Indisponível' : number(diskHealth.readErrorsUncorrected)}</strong></div>
                <div><small>Erros de gravação não corrigidos</small><strong>{diskHealth.writeErrorsUncorrected == null ? 'Indisponível' : number(diskHealth.writeErrorsUncorrected)}</strong></div>
                <div><small>Horas ligado</small><strong>{diskHealth.powerOnHours == null ? 'Indisponível' : number(diskHealth.powerOnHours)}</strong></div>
                <div><small>Estado operacional</small><strong>{diskHealth.operationalStatus || 'Indisponível'}</strong></div>
              </div>
              {!diskHealth.reliabilityAvailable && <div className="alert warning-alert health-warning"><Info size={17}/><span>O Windows não disponibilizou contadores de confiabilidade para esta unidade. Um estado geral saudável não substitui o diagnóstico SMART do fabricante.</span></div>}
            </> : <div className="health-empty"><Activity size={21}/> Selecione a letra da unidade abaixo e consulte seus indicadores. Alguns dispositivos não disponibilizam todos os sensores.</div>}
            <p className="panel-note"><ShieldCheck size={14}/> Valores ausentes não significam zero; Barramento NVMe/SATA não determina automaticamente o tipo de mídia. Não inferimos vida útil restante.</p>
          </section>
          <section className="panel glass optimize-panel"><SectionHeading kicker="ADAPTADOR WINDOWS" title="Analisar ou otimizar unidade" description="Primeiro execute a análise. A otimização exige confirmação explícita e pode exigir privilégios de administrador."/>
            <div className="optimize-controls"><label className="field"><span>Letra da unidade</span><div className="input-wrap drive-input"><Disc3 size={19}/><input maxLength={2} value={drive} onChange={(e) => { setDrive(e.target.value.toUpperCase()); setDiskHealth(null); setOptimization(null); setConfirmOptimize(false); }} aria-label="Letra da unidade"/><strong>:</strong></div></label><button className="outline-button" type="button" disabled={optimizing} onClick={() => void runOptimization(false)}>{optimizing ? <LoaderCircle className="spin" size={17}/> : <Activity size={17}/>} Analisar volume</button></div>
            <label className="confirm-box"><input type="checkbox" checked={confirmOptimize} onChange={(e) => setConfirmOptimize(e.target.checked)}/><span>Entendo que a otimização modifica a disposição física/lógica de dados do volume e pode exigir administrador. Fiz backup dos dados importantes.</span></label>
            <button className="primary-button optimize-action" type="button" disabled={optimizing || !confirmOptimize || !optimization || optimization.executed || optimization.drive !== drive.trim().replace(':', '').toUpperCase()} onClick={() => void runOptimization(true)}><WandSparkles size={18}/> Iniciar otimização pelo Windows <ArrowRight size={17}/></button>
            {optimization && <div className="command-output"><strong><Check size={17}/> {optimization.executed ? 'Otimização solicitada' : 'Análise concluída'} — unidade {optimization.drive}:</strong><pre>{optimization.output}</pre></div>}
          </section>
          <div className="safety-grid"><div className="safety-card glass"><ShieldCheck size={21}/><strong>Sem alteração automática</strong><p>Nenhuma execução é disparada pela análise de arquivos.</p></div><div className="safety-card glass"><LockKeyhole size={21}/><strong>Sem privilégios ocultos</strong><p>O sistema não contorna permissões do Windows.</p></div><div className="safety-card glass"><Info size={21}/><strong>Limites da versão</strong><p>O adaptador de otimização está disponível apenas no Windows.</p></div></div>
        </div>}

        <div className="bottom-status"><span><span className="live-dot"/> PROCESSAMENTO LOCAL ATIVO</span><span>{scanned ? number(report.filesScanned) + ' ARQUIVOS · ' + duration(report.elapsedMs) : 'PRONTO PARA ANALISAR'} </span><span>THORN INTELLIGENCE · 0.1.0</span></div>
      </main>
    </div>
  </div>;
}
