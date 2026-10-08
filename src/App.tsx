import { useEffect, useRef, useState, type ComponentType, type FormEvent, type ReactNode } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import { Effect, EffectState, getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import {
  Activity, AlertCircle, ArrowRight, ArrowUpRight,
  BarChart3, Check, CheckCircle2, ChevronRight, CircleHelp, Clipboard,
  Database, Disc3, File, FileSearch, Fingerprint, Folder, FolderOpen,
  Gauge, HardDrive, Info, Layers3, LoaderCircle, LockKeyhole, Menu, ArchiveRestore, CloudOff,
  Search, ShieldCheck, SlidersHorizontal, Sparkles, WandSparkles, X,
} from 'lucide-react';
import type { IndexedScope, IndexStats, IndexedSearch, QuarantinePreview, QuarantineItem, CompareReport, CompareRequest, AllocationItem, AllocationReport, AllocationRequest, DiskHealth, DuplicateGroup, FileResult, OptimizationResult, ScanReport, ScanProgress, ScanRequest, SearchRequest, SearchReport, Section } from './types';
import { bytes, duration, number, truncatePath } from './lib/format';
import { IndexedTree } from './components/IndexedTree';

type IconType = ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
const links: { id: Section; label: string; icon: IconType }[] = [
  { id: 'overview', label: 'Visão geral', icon: BarChart3 },
  { id: 'explorer', label: 'Explorador', icon: FolderOpen },
  { id: 'duplicates', label: 'Duplicados', icon: Layers3 },
  { id: 'compare', label: 'Comparar pastas', icon: Fingerprint },
  { id: 'cleanup', label: 'Quarentena segura', icon: ArchiveRestore },
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

function FileRows({ files, copy, allocations }: {
  files: FileResult[];
  copy: (path: string) => void;
  allocations?: Record<string, AllocationItem>;
}) {
  if (!files.length) return <div className="empty-list">Nenhum arquivo encontrado nesta visualização.</div>;
  return <div className="file-table">
    <div className="file-table-head"><span>ARQUIVO</span><span>TIPO</span><span>TAMANHO</span><span></span></div>
    {files.map((file) => <div className="file-row" key={file.path}>
      <span className="file-identity"><span className="file-icon"><File size={18}/></span><span className="file-text"><strong title={file.name}>{file.name}</strong><small title={file.path}>{truncatePath(file.path, 64)}</small></span></span>
      <span className="type-cell">{file.extension}
        {file.contentStatus === 'offline' && <small className="storage-status remote" title="Metadados indicam armazenamento remoto/offline; o aplicativo não abriu o conteúdo">Remoto/offline</small>}
        {file.contentStatus === 'reparse' && <small className="storage-status reparse" title="Arquivo virtual ou redirecionado (reparse point); conteúdo não foi aberto">Redirecionado</small>}
      </span><span className="size-cell"><strong>{bytes(file.sizeBytes)}</strong>
        {allocations?.[file.path] && <small className="allocation-detail" title={
          allocations[file.path].status === 'measured'
            ? 'Alocação do filesystem via FileStandardInfo: não é espaço recuperável'
            : 'Medição omitida: arquivo alterado, virtual, fora do escopo ou inacessível'
        }>{allocations[file.path].allocatedBytes !== null
            ? 'Em disco: ' + bytes(allocations[file.path].allocatedBytes as number)
            : 'Em disco: indisponível'}</small>}
        {(allocations?.[file.path]?.hardlinkCount ?? 0) > 1 && <small
          className="allocation-detail" title="Há outras referências físicas ao mesmo arquivo, possivelmente fora desta pasta. Remover um único nome não recupera esse espaço.">
          Compartilhado: {allocations?.[file.path]?.hardlinkCount} hardlinks · economia não atribuível
        </small>}
      </span>
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
  const [searchResult, setSearchResult] = useState<SearchReport | null>(null);
  const [searchBusy, setSearchBusy] = useState(false);
  const activeJob = useRef<string | null>(null);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [busy, setBusy] = useState(false);
  const [includeDuplicates, setIncludeDuplicates] = useState(false);
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
  const [cloudFilter, setCloudFilter] = useState<'all' | 'offline' | 'reparse'>('all');
  const [allocationBusy, setAllocationBusy] = useState(false);
  const [allocationReport, setAllocationReport] = useState<AllocationReport | null>(null);
  const [compareBusy, setCompareBusy] = useState(false);
  const [referenceRoot, setReferenceRoot] = useState('');
  const [candidateRoot, setCandidateRoot] = useState('');
  const [comparison, setComparison] = useState<CompareReport | null>(null);
  const [indexBusy, setIndexBusy] = useState(false);
  const [indexPaused, setIndexPaused] = useState(false);
  const [indexPausePending, setIndexPausePending] = useState(false);
  const [indexStats, setIndexStats] = useState<IndexStats | null>(null);
  const [indexedScopes, setIndexedScopes] = useState<IndexedScope[]>([]);
  const [selectedIndexedRoot, setSelectedIndexedRoot] = useState('');
  const [indexedScopesLoading, setIndexedScopesLoading] = useState(false);
  const [useCachedIndex, setUseCachedIndex] = useState(false);
  const [cachedAt, setCachedAt] = useState<number | null>(null);
  const [quarantinePath, setQuarantinePath] = useState('');
  const [preview, setPreview] = useState<QuarantinePreview | null>(null);
  const [quarantined, setQuarantined] = useState<QuarantineItem[]>([]);
  const [moveConfirmation, setMoveConfirmation] = useState('');
  const [restoreConfirmation, setRestoreConfirmation] = useState('');
  const [quarantineBusy, setQuarantineBusy] = useState(false);
  const [mobileMenu, setMobileMenu] = useState(false);
  const [windowEffectStatus, setWindowEffectStatus] = useState<'pending' | 'requested' | 'failed'>('pending');
  const [windowEffectError, setWindowEffectError] = useState('');

  // Acrylic is the visibly translucent Windows 10/11 backdrop; Mica is
  // a native Windows 11 fallback, not the same as seeing the desktop.
  // Tauri's window must have transparent:true, and every WebView root
  // layer must be translucent *before* requesting the Windows backdrop.
  useEffect(() => {
    if (!navigator.userAgent.includes('Windows')) return;
    let mounted = true;
    const page = document.documentElement;
    page.classList.add('native-vibrancy');
    const apply = async () => {
      try {
        // Windows first-paint workaround (tauri#8632): start undecorated
        // with no shadow, restore the native titlebar before requesting DWM.
        const nativeWindow = getCurrentWindow();
        await nativeWindow.setDecorations(true);
        if (!mounted) return;
        await nativeWindow.setEffects({
          effects: [Effect.Acrylic, Effect.Mica],
          state: EffectState.Active,
        });
        if (mounted) setWindowEffectStatus('requested');
      } catch (err) {
        // Previously swallowed every error, leaving an unexplained solid UI.
        console.warn('[Thorn Intelligence] Native Windows backdrop unavailable:', err);
        if (mounted) {
          page.classList.remove('native-vibrancy');
          setWindowEffectError(String(err));
          setWindowEffectStatus('failed');
        }
      }
    };
    void apply();
    return () => {
      mounted = false;
      page.classList.remove('native-vibrancy');
    };
  }, []);

  async function copy(value: string) {
    try { await navigator.clipboard.writeText(value); setToast('Caminho copiado.'); }
    catch { setToast('Não foi possível copiar o caminho.'); }
  }

  async function scanFolder(root?: string, pattern = regex, min = minMb, analyze = includeDuplicates) {
    if (activeJob.current || busy || searchBusy || allocationBusy || compareBusy || indexBusy) {
      setError('Já existe uma análise em andamento.');
      return;
    }
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
    const jobId = crypto.randomUUID();
    const onProgress = new Channel<ScanProgress>();
    onProgress.onmessage = (event) => {
      if (activeJob.current === jobId) setScanProgress(event);
    };
    activeJob.current = jobId;
    setScanProgress(null);
    setCancelRequested(false);
    setBusy(true);
    try {
      const parsed = Number(min);
      if (!Number.isFinite(parsed) || parsed < 0) throw new Error('Tamanho mínimo inválido.');
      const request: ScanRequest = { root: path, regex: pattern.trim() || null, minSizeBytes: Math.floor(parsed * 1024 * 1024), analyzeDuplicates: analyze };
      const result = await invoke<ScanReport>('scan_path', { request, jobId, onProgress });
      setReport(result);
      setSelectedIndexedRoot(result.root);
      setSearchResult(null);
      setAllocationReport(null);
      setToast('Análise concluída em ' + duration(result.elapsedMs) + '.');
    } catch (err) {
      const message = String(err);
      if (message.includes('cancelada')) setToast('Varredura cancelada; o relatório anterior foi preservado.');
      else setError(message);
    } finally {
      if (activeJob.current === jobId) activeJob.current = null;
      setScanProgress(null);
      setCancelRequested(false);
      setBusy(false);
    }
  }

  async function searchMetadata() {
    if (!report) return;
    if (activeJob.current || busy || searchBusy || allocationBusy || compareBusy || indexBusy) {
      setError('Já existe uma operação em andamento.');
      return;
    }
    setError('');
    const jobId = crypto.randomUUID();
    const onProgress = new Channel<ScanProgress>();
    onProgress.onmessage = (event) => {
      if (activeJob.current === jobId) setScanProgress(event);
    };
    activeJob.current = jobId;
    setScanProgress(null);
    setCancelRequested(false);
    setSearchBusy(true);
    try {
      const parsed = Number(minMb);
      if (!Number.isFinite(parsed) || parsed < 0 || parsed > Number.MAX_SAFE_INTEGER / 1048576) {
        throw new Error('Tamanho mínimo inválido.');
      }
      const request: SearchRequest = {
        root: report.root,
        regex: regex.trim() || null,
        minSizeBytes: Math.floor(parsed * 1048576),
       };
      if (useCachedIndex) {
        const cached = await invoke<IndexedSearch>('search_index', { request });
        setSearchResult(cached.report);
        setCachedAt(cached.completedAtUnix);
        setToast('Busca no índice SQLite concluída; resultados referentes ao último snapshot.');
      } else {
        const result = await invoke<SearchReport>('search_path', { request, jobId, onProgress });
        setSearchResult(result);
        setCachedAt(null);
        setToast('Busca por metadados concluída em ' + duration(result.elapsedMs) + '.');
      }
    } catch (err) {
      const message = String(err);
      if (message.includes('cancelada')) setToast('Pesquisa cancelada; os resultados anteriores foram preservados.');
      else setError(message);
    } finally {
      if (activeJob.current === jobId) activeJob.current = null;
      setScanProgress(null);
      setCancelRequested(false);
      setSearchBusy(false);
    }
  }

  async function measureAllocated() {
    if (!report || !report.topFiles.length || activeJob.current || busy || searchBusy || allocationBusy || compareBusy || indexBusy) return;
    setError('');
    const jobId = crypto.randomUUID();
    const onProgress = new Channel<ScanProgress>();
    onProgress.onmessage = (event) => {
      if (activeJob.current === jobId) setScanProgress(event);
    };
    activeJob.current = jobId;
    setScanProgress(null);
    setCancelRequested(false);
    setAllocationBusy(true);
    try {
      const request: AllocationRequest = {
        root: report.root,
        targets: report.topFiles.slice(0, 300).map(file => ({
          path: file.path, expectedSizeBytes: file.sizeBytes,
        })),
      };
      const result = await invoke<AllocationReport>('measure_allocated_sizes', {
        request, jobId, onProgress,
      });
      setAllocationReport(result);
      setToast('Alocação consultada em ' + duration(result.elapsedMs)
        + ' · ' + number(result.measured) + ' arquivos medidos.');
    } catch (err) {
      const message = String(err);
      if (message.includes('cancelada')) setToast('Medição cancelada; os resultados anteriores foram preservados.');
      else setError('Não foi possível medir o espaço alocado: ' + message);
    } finally {
      if (activeJob.current === jobId) activeJob.current = null;
      setScanProgress(null);
      setCancelRequested(false);
      setAllocationBusy(false);
    }
  }

  async function pickComparisonFolder(kind: 'reference' | 'candidate') {
    if (busy || searchBusy || compareBusy || activeJob.current) return;
    setError('');
    try {
      const selected = await open({
        directory: true, multiple: false,
        title: kind === 'reference' ? 'Escolher pasta mestre (preservar)' : 'Escolher pasta para conferir',
      });
      if (typeof selected !== 'string') return;
      if (kind === 'reference') setReferenceRoot(selected);
      else setCandidateRoot(selected);
      setComparison(null);
    } catch (err) {
      setError('Não foi possível escolher a pasta: ' + String(err));
    }
  }

  async function compareFolders() {
    if (!referenceRoot || !candidateRoot || activeJob.current || busy || searchBusy || compareBusy) return;
    setError('');
    const jobId = crypto.randomUUID();
    const onProgress = new Channel<ScanProgress>();
    onProgress.onmessage = (event) => {
      if (activeJob.current === jobId) setScanProgress(event);
    };
    activeJob.current = jobId;
    setScanProgress(null);
    setCancelRequested(false);
    setCompareBusy(true);
    try {
      const request: CompareRequest = {
        referenceRoot, candidateRoot, maxFiles: 250_000,
      };
      const result = await invoke<CompareReport>('compare_folders', { request, jobId, onProgress });
      setComparison(result);
      setToast('Comparação concluída em ' + duration(result.elapsedMs) +
        (result.complete ? '.' : ' · resultado parcial.'));
    } catch (err) {
      const message = String(err);
      if (message.includes('cancelada')) {
        setToast('Comparação cancelada. O relatório anterior foi preservado.');
      } else setError(message);
    } finally {
      if (activeJob.current === jobId) activeJob.current = null;
      setScanProgress(null);
      setCancelRequested(false);
      setCompareBusy(false);
    }
  }

  async function cancelCurrentScan() {
    const jobId = activeJob.current;
    if (!jobId) return;
    setCancelRequested(true);
    try {
      const accepted = await invoke<boolean>('cancel_scan', { jobId });
      if (!accepted) setToast('Operação já finalizada.');
    } catch (err) {
      setError('Não foi possível cancelar a operação: ' + String(err));
      setCancelRequested(false);
    }
  }

  async function toggleIndexPause() {
    const jobId = activeJob.current;
    if (!jobId || !indexBusy || indexPausePending || cancelRequested) return;
    const shouldPause = !indexPaused;
    setIndexPausePending(true);
    try {
      const accepted = await invoke<boolean>(shouldPause ? 'pause_index' : 'resume_index', { jobId });
      if (accepted) setIndexPaused(shouldPause);
      else setError('A tarefa já terminou ou não aceita pausa.');
    } catch (err) {
      setError('Não foi possível alterar o estado da indexação: ' + String(err));
    } finally {
      setIndexPausePending(false);
    }
  }

  async function loadIndexedScopes() {
    setIndexedScopesLoading(true);
    try {
      const scopes = await invoke<IndexedScope[]>('list_indexed_scopes');
      setIndexedScopes(scopes);
      setSelectedIndexedRoot(previous =>
        previous && scopes.some(item => item.root === previous) ? previous
          : report?.root || scopes[0]?.root || ''
      );
    } catch (err) {
      setError('Não foi possível listar os índices locais: ' + String(err));
    } finally {
      setIndexedScopesLoading(false);
    }
  }

  async function refreshIndex(targetRoot?: string) {
    const root = targetRoot || selectedIndexedRoot || report?.root || '';
    if (!root || activeJob.current || busy || searchBusy || indexBusy) return;
    setError('');
    const jobId = crypto.randomUUID();
    const onProgress = new Channel<ScanProgress>();
    onProgress.onmessage = (event) => {
      if (activeJob.current === jobId) {
        setScanProgress(event);
        if (event.phase === 'paused') setIndexPaused(true);
        if (event.phase === 'indexing') setIndexPaused(false);
      }
    };
    activeJob.current = jobId;
    setScanProgress(null);
    setCancelRequested(false);
    setIndexPaused(false);
    setIndexPausePending(false);
    setIndexBusy(true);
    try {
      const stats = await invoke<IndexStats>('refresh_index', {
        root, jobId, onProgress,
      });
      setIndexStats(stats);
      setSelectedIndexedRoot(stats.root);
      await loadIndexedScopes();
      setUseCachedIndex(true);
      setToast('Índice atualizado: ' + number(stats.added) + ' novos, ' +
        number(stats.changed) + ' alterados, ' + number(stats.unchanged) + ' inalterados.');
    } catch (err) {
      const message = String(err);
      if (message.includes('cancelada')) setToast('Indexação cancelada; último snapshot preservado.');
      else setError(message);
    } finally {
      if (activeJob.current === jobId) activeJob.current = null;
      setScanProgress(null);
      setCancelRequested(false);
      setIndexPaused(false);
      setIndexPausePending(false);
      setIndexBusy(false);
    }
  }

  async function pickQuarantineFile() {
    if (quarantineBusy) return;
    try {
      const path = await open({ directory: false, multiple: false,
        title: 'Escolha um arquivo local para pré-visualizar' });
      if (typeof path === 'string') {
        setQuarantinePath(path); setPreview(null); setMoveConfirmation('');
      }
    } catch (err) { setError(String(err)); }
  }

  async function loadQuarantine() {
    try {
      const entries = await invoke<QuarantineItem[]>('list_quarantine');
      setQuarantined(entries);
    } catch (err) { setError('Não foi possível listar a quarentena: ' + String(err)); }
  }

  async function previewQuarantine() {
    if (quarantineBusy) return;
    setError(''); setPreview(null); setMoveConfirmation(''); setQuarantineBusy(true);
    try {
      const next = await invoke<QuarantinePreview>('preview_quarantine', { path: quarantinePath });
      setPreview(next);
    } catch (err) { setError('Pré-visualização bloqueada: ' + String(err)); }
    finally { setQuarantineBusy(false); }
  }

  async function moveToQuarantine() {
    if (quarantineBusy || !preview || moveConfirmation !== 'MOVER PARA QUARENTENA') return;
    setError(''); setQuarantineBusy(true);
    try {
      await invoke<QuarantineItem>('quarantine_file', {
        previewId: preview.previewId, confirmation: moveConfirmation,
      });
      setPreview(null); setMoveConfirmation(''); setQuarantinePath('');
      await loadQuarantine();
      setToast('Arquivo movido para a quarentena local. Nenhum espaço físico foi liberado.');
    } catch (err) {
      setPreview(null); setMoveConfirmation('');
      setError('Movimentação não confirmada: ' + String(err) +
        '. Consulte a lista da quarentena antes de tentar novamente.');
      await loadQuarantine();
    } finally { setQuarantineBusy(false); }
  }

  async function restoreQuarantine(id: string) {
    if (quarantineBusy || restoreConfirmation !== 'RESTAURAR') return;
    setError(''); setQuarantineBusy(true);
    try {
      await invoke<QuarantineItem>('restore_quarantine', {
        id, confirmation: restoreConfirmation,
      });
      setRestoreConfirmation('');
      await loadQuarantine();
      setToast('Arquivo restaurado ao caminho original.');
    } catch (err) {
      setError('Restauração bloqueada: ' + String(err));
      await loadQuarantine();
    } finally { setQuarantineBusy(false); }
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
    if (value === 'cleanup') void loadQuarantine();
    if (value === 'explorer') void loadIndexedScopes();
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
        <div className="top-actions">
          {navigator.userAgent.includes('Windows') && <span
            className={'effect-pill ' + (windowEffectStatus === 'failed' ? 'is-unavailable' : '')}
            role="status"
            title={windowEffectStatus === 'failed'
              ? 'Mica/Acrylic não puderam ser solicitados: ' + windowEffectError
              : windowEffectStatus === 'requested'
                ? 'Efeito solicitado ao Windows. A aparência final depende dos efeitos de transparência do sistema, WebView2 e DWM.'
                : 'Inicializando o efeito nativo do Windows.'}>
            {windowEffectStatus === 'failed' ? 'Vidro indisponível'
              : windowEffectStatus === 'requested' ? 'Acrylic solicitado'
              : 'Ativando vidro…'}
          </span>}
          <span className="local-pill"><span className="live-dot"/> LOCAL-FIRST</span><button className="top-help" onClick={() => selectSection('optimize')} title="Conheça os controles de segurança" aria-label="Segurança"><CircleHelp size={19}/></button><div className="user-avatar"><Disc3 size={17}/></div></div>
      </header>
      <main id="main-content" className="content">
        <div className="hero-heading">
          <div><div className="hero-kicker"><Sparkles size={14}/> ARMAZENAMENTO SOB CONTROLE</div><h1>{links.find((link) => link.id === section)?.label}<span className="heading-period">.</span></h1><p>Descubra o que ocupa espaço, identifique desperdícios e tome decisões com segurança.</p></div>
          <div className="scan-controls">
            <label className="scan-hash-option" title="Modo rápido: só inventário. Ative para detectar duplicados por BLAKE3.">
              <input type="checkbox" checked={includeDuplicates} disabled={busy || searchBusy || indexBusy}
                onChange={event => setIncludeDuplicates(event.target.checked)}/>
              <span>Incluir BLAKE3 <small>{includeDuplicates ? 'Varredura completa (mais I/O)' : 'Desativado: modo rápido'}</small></span>
            </label>
            <button className="primary-button" disabled={busy || searchBusy || allocationBusy || compareBusy || indexBusy} onClick={() => void scanFolder()}>{busy ? <LoaderCircle className="spin" size={18}/> : <FolderOpen size={18}/>} {busy ? 'Analisando…' : 'Analisar pasta'} <ArrowRight size={16}/></button>
          </div>
        </div>

        <div className="scope-strip glass"><div className="scope-icon"><Folder size={19}/></div><div className="scope-details"><small>ESCOPO ATUAL</small><strong title={roots}>{scopeName}</strong></div><span className="scope-full" title={roots}>{scanned ? truncatePath(roots, 56) : 'Escolha uma pasta ou unidade para iniciar'}</span><Tag tone={scanned ? 'green' : 'neutral'}>{scanned ? 'ANALISADO' : 'AGUARDANDO'}</Tag></div>

        {(busy || searchBusy || allocationBusy || compareBusy || indexBusy) && <div className="scan-activity glass" role="status" aria-live="polite">
          <div className="activity-spinner"><LoaderCircle size={20} className="spin"/></div>
          <div className="activity-details">
            <strong>{scanProgress?.phase === 'hashing' ? 'Verificando duplicados com BLAKE3' :
              scanProgress?.phase === 'verifying' ? 'Conferindo identidades físicas' :
              scanProgress?.phase === 'searching' ? 'Pesquisando metadados' :
              scanProgress?.phase === 'paused' ? 'Indexação pausada pelo usuário' :
              scanProgress?.phase === 'indexing' ? 'Atualizando índice SQLite' :
              scanProgress?.phase === 'comparing' ? 'Inventariando as duas pastas' :
              scanProgress?.phase === 'allocation' ? 'Consultando alocação no Windows' :
              scanProgress?.phase === 'fingerprinting' ? 'Comparando amostras de 16 KiB' :
              'Analisando diretórios'}</strong>
            <small>{number(scanProgress?.filesScanned ?? 0)} {
              scanProgress?.phase === 'allocation' ? 'arquivos medidos/consultados' :
              scanProgress?.phase === 'hashing' ? 'candidatos com BLAKE3 completo' :
              scanProgress?.phase === 'fingerprinting' ? 'candidatos amostrados' :
              'arquivos enumerados'
            }{scanProgress && scanProgress.hashBytesRead > 0
              ? ' · ' + bytes(scanProgress.hashBytesRead) + ' lidos no total (amostras + BLAKE3)'
              : ''}
            </small>
          </div>
          {indexBusy && <button type="button" className="outline-button"
            disabled={!scanProgress || indexPausePending || cancelRequested}
            aria-label={indexPaused ? 'Retomar indexação' : 'Pausar indexação'}
            onClick={() => void toggleIndexPause()}>
            {indexPausePending ? 'Aguarde…' : indexPaused ? 'Retomar' : 'Pausar'}
          </button>}
          <button type="button" className="outline-button" disabled={cancelRequested || !scanProgress}
            title={!scanProgress ? 'Aguardando inicialização no backend' : 'Interromper a operação atual'}
            onClick={() => void cancelCurrentScan()}>
            {cancelRequested ? 'Cancelando…' : 'Cancelar'}
          </button>
        </div>}
        {error && <div role="alert" className="alert error-alert"><AlertCircle size={19}/><span>{error}</span><button aria-label="Fechar aviso" className="icon-button" onClick={() => setError('')}><X size={16}/></button></div>}
        {toast && <div role="status" className="alert toast-alert"><Check size={17}/><span>{toast}</span><button aria-label="Fechar mensagem" className="icon-button" onClick={() => setToast('')}><X size={16}/></button></div>}
        {report && (report.truncated || report.errors > 0 || !report.duplicateAnalysisComplete || report.hardlinkAliases > 0 || report.skippedContentFiles > 0) && <div className="alert warning-alert"><Info size={18}/><span>{report.truncated ? 'Amostragem solicitada: limite explícito de arquivos atingido; o relatório é parcial. ' : ''}{report.errors > 0 ? number(report.errors) + ' entradas não puderam ser processadas. ' : ''}{report.hashingSkipped ? 'Modo rápido: BLAKE3 não executado. Ative a análise para verificar cópias. ' :
            !report.duplicateAnalysisComplete ? 'Análise de duplicados incompleta (limite de leitura ou arquivos indisponíveis); pode haver mais cópias. ' : ''}{report.hardlinkAliases > 0 ? number(report.hardlinkAliases) + ' links físicos compartilhados foram excluídos das estimativas. ' : ''}{report.skippedContentFiles > 0 ? number(report.skippedContentFiles) + ' arquivos de conteúdo remoto/offline ou reparse foram ignorados no hash para evitar downloads involuntários. ' : ''}As estimativas não equivalem a espaço liberado.</span></div>}

        {!scanned && section !== 'optimize' && section !== 'compare' && section !== 'cleanup' && section !== 'explorer' && <div className="onboarding glass">
          <div className="onboarding-content"><Tag tone="blue"><Sparkles size={13}/> INTELLIGENT STORAGE</Tag><h2>Encontre espaço que você nem sabia que tinha.</h2><p>Mapeie arquivos, compare tamanhos, descubra duplicados reais com BLAKE3 e pesquise nomes ou caminhos usando expressões regulares. Tudo acontece no seu computador.</p><button className="primary-button" disabled={busy} onClick={() => void scanFolder()}><FolderOpen size={18}/> Escolher pasta <ArrowRight size={17}/></button></div>
          <div className="onboarding-visual"><div className="orb orb-one"/><div className="orb orb-two"/><div className="preview-card preview-main"><div className="preview-dotline"><i/><i/><i/></div><div className="preview-chart"><div/><div/><div/><div/><div/><div/><div/></div><div className="preview-baseline"/></div><div className="preview-card preview-small"><Fingerprint size={21}/><span>BLAKE3</span><CheckCircle2 size={17}/></div></div>
        </div>}

        {section === 'overview' && report && <>
          <div className="metrics-grid">
            <Metric label="Volume analisado" value={bytes(report.logicalBytes)} helper="Tamanho lógico dos arquivos" icon={Database}/>
            <Metric label="Arquivos analisados" value={number(report.filesScanned)} helper={number(report.directoriesScanned) + ' diretórios percorridos'} icon={FileSearch} tone="violet"/>
            <Metric label="Espaço duplicado" value={report.hashingSkipped ? '—' : bytes(report.potentialSavingsBytes)} helper={report.hashingSkipped ? 'BLAKE3 ainda não executado' : 'Estimativa, sem exclusões'} icon={Fingerprint} tone="mint"/>
            <Metric label="Grupos duplicados" value={report.hashingSkipped ? '—' : number(report.duplicates.length)} helper={report.hashingSkipped ? 'Ative o modo completo' : bytes(report.hashBytesRead) + ' lidos por hash'} icon={Layers3} tone="pink"/>
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

        {section === 'explorer' && <section className="panel glass full-panel">
          <SectionHeading kicker="NAVEGAÇÃO PERSISTENTE" title="Árvore de pesquisa SQLite"
            description="Abra índices já salvos, mesmo depois de reiniciar o aplicativo. Não é necessário analisar todos os arquivos novamente."/>
          <div className="indexed-scopes-controls">
            <label className="field">
              <span>Escopo indexado</span>
              <select aria-label="Escolher escopo indexado"
                disabled={indexedScopesLoading || indexBusy}
                value={selectedIndexedRoot || report?.root || ''}
                onChange={event => setSelectedIndexedRoot(event.target.value)}>
                {!selectedIndexedRoot && !report && <option value="">Escolha um índice salvo</option>}
                {(selectedIndexedRoot || report?.root) &&
                 !indexedScopes.some(item => item.root === (selectedIndexedRoot || report?.root)) &&
                  <option value={selectedIndexedRoot || report?.root || ''}>
                    {truncatePath(selectedIndexedRoot || report?.root || '', 90)} — requer indexação
                  </option>}
                {indexedScopes.map(item =>
                  <option key={item.root} value={item.root}>
                    {truncatePath(item.root, 90)} · {number(item.files)} arquivos
                    {item.hasTree ? '' : ' · atualizar para ativar árvore'}
                  </option>)}
              </select>
            </label>
            <button type="button" className="outline-button"
              disabled={indexedScopesLoading} onClick={() => void loadIndexedScopes()}>
              {indexedScopesLoading ? 'Consultando…' : 'Recarregar índices'}
            </button>
          </div>
          {(selectedIndexedRoot || report?.root) ?
            <IndexedTree root={selectedIndexedRoot || report?.root || ''}
              revision={indexStats?.root === (selectedIndexedRoot || report?.root)
                ? indexStats.completedAtUnix : 0}
              refreshing={indexBusy}
              onRefresh={() => void refreshIndex(selectedIndexedRoot || report?.root)}
              onCopy={(path) => void copy(path)}/>
            : <p className="panel-note">Nenhum índice publicado. Faça uma análise de pasta e use
                Atualizar índice SQLite na busca para criar um snapshot persistente.</p>}
        </section>}

        {section === 'explorer' && report &&
         (!selectedIndexedRoot || selectedIndexedRoot === report.root) &&
         <section className="panel glass full-panel">
          <SectionHeading kicker="RANKING POR TAMANHO" title="Arquivos grandes"
            description="Os 300 maiores arquivos encontrados, com identificação de conteúdo offline e redirecionamentos feita apenas pelos metadados."
            right={<div className="allocation-actions">
              <Tag tone="blue">{report.topFiles.length} resultados</Tag>
              <button type="button" className="outline-button" disabled={busy || searchBusy || allocationBusy || compareBusy || indexBusy}
                onClick={() => void measureAllocated()}>
                {allocationBusy ? <LoaderCircle size={16} className="spin"/> : <HardDrive size={16}/>}
                {allocationBusy ? 'Medindo…' : 'Medir espaço em disco'}
              </button>
            </div>}/>
          <div className="storage-filters" role="group" aria-label="Filtrar por disponibilidade de arquivo">
            {([
              ['all', 'Todos'],
              ['offline', 'Remoto / offline'],
              ['reparse', 'Redirecionados'],
            ] as const).map(([status, label]) =>
              <button key={status} type="button" aria-pressed={cloudFilter === status}
                className={'storage-filter ' + (cloudFilter === status ? 'active' : '')}
                onClick={() => setCloudFilter(status)}>{status !== 'all' && <CloudOff size={14}/>}
                {label}</button>
            )}
          </div>
          <p className="panel-note">O filtro atua sobre os 300 maiores arquivos do relatório.
            O status remoto é inferido por atributos do Windows; não exige download do arquivo.
            Outros arquivos virtuais podem não apresentar todos esses atributos.</p>
          {allocationReport && <p className="panel-note">
            Alocação consultada em {number(allocationReport.measured)} arquivos;
            {number(allocationReport.skipped)} ignorados e {number(allocationReport.failed)} indisponíveis.
            {allocationReport.items.filter(item => (item.hardlinkCount ?? 0) > 1).length > 0 &&
              ' ' + number(allocationReport.items.filter(item => (item.hardlinkCount ?? 0) > 1).length) + ' arquivos possuem hardlinks compartilhados.'}
            Valores restritos à lista dos 300 maiores — não são o espaço físico total da pasta
            nem equivalem a espaço recuperável. Hardlinks podem existir fora do escopo;
            compressão, arquivos esparsos ou armazenamento compartilhado alteram a alocação reportada.
          </p>}
          <FileRows files={report.topFiles.filter(file =>
            cloudFilter === 'all' || file.contentStatus === cloudFilter
          )} copy={copy} allocations={allocationReport
            ? Object.fromEntries(allocationReport.items.map(item => [item.path, item]))
            : undefined}/>
        </section>}

        {section === 'duplicates' && report && <div className="stack-gap">
          {report.hashingSkipped && <div className="alert warning-alert">
            <Info size={18}/><span>Esta pasta foi analisada no modo rápido (somente metadados).
              Nenhuma verificação de duplicados foi executada.</span>
            <button className="outline-button" type="button" disabled={busy || searchBusy || indexBusy}
              onClick={() => { setIncludeDuplicates(true); void scanFolder(report.root, regex, minMb, true); }}>
              Executar BLAKE3</button>
          </div>}
          <div className="insight-banner glass"><div className="insight-icon"><Fingerprint size={25}/></div><div><small>DUPLICAÇÃO VERIFICADA</small><strong>{report.hashingSkipped ? 'Aguardando BLAKE3' : bytes(report.potentialSavingsBytes) + ' de economia potencial'}</strong><p>Apenas cópias independentes, com mesmo tamanho e hash BLAKE3 idêntico, aparecem abaixo. Hardlinks foram excluídos das economias estimadas. Nenhum arquivo é removido.</p></div><Tag tone={report.hashingSkipped ? 'amber' : report.duplicateAnalysisComplete ? 'green' : 'amber'}>{report.hashingSkipped ? 'NÃO EXECUTADO' : report.duplicateAnalysisComplete ? 'HASH COMPLETO*' : 'HASH PARCIAL'}</Tag></div>
          <SectionHeading kicker="INSPEÇÃO MANUAL" title="Grupos idênticos" description="Copie os caminhos e revise antes de qualquer intervenção."/>
          {report.duplicates.length ? report.duplicates.map((group) => <DuplicateCard group={group} key={group.hash} copy={copy}/>) : <div className="empty-list standalone">{report.hashingSkipped ? 'A análise BLAKE3 ainda não foi solicitada.' : 'Nenhum grupo duplicado confirmado dentro do orçamento de hash.'}</div>}
          <p className="subnote">* Completo dentro do escopo analisado, sujeito a erros de leitura, limite de arquivos e links físicos.</p>
        </div>}

        {section === 'compare' && <div className="stack-gap">
          <div className="insight-banner glass">
            <div className="insight-icon"><Fingerprint size={25}/></div>
            <div>
              <small>COMPARAÇÃO MESTRE → CANDIDATA</small>
              <strong>Encontre cópias sem mexer no acervo original.</strong>
              <p>Selecione duas pastas independentes. O Thorn compara primeiro os tamanhos, depois confirma o conteúdo
                com BLAKE3 e exclui hardlinks da estimativa. Nunca move ou exclui arquivos nesta tela.</p>
            </div>
            <Tag tone="green">SOMENTE LEITURA</Tag>
          </div>
          <section className="panel glass">
            <SectionHeading kicker="ESCOLHA AS PASTAS" title="Comparação orientada por referência"
              description="Pasta mestre: tudo é preservado. Pasta candidata: somente nela são identificadas cópias existentes na mestre."/>
            <div className="compare-folder-list">
              <div className="compare-folder-row">
                <div className="compare-folder-text">
                  <strong>Pasta mestre · preservar</strong>
                  <small title={referenceRoot}>{referenceRoot || 'Selecione o acervo principal'}</small>
                </div>
                <button type="button" className="outline-button" disabled={compareBusy || busy || searchBusy}
                  onClick={() => void pickComparisonFolder('reference')}><FolderOpen size={16}/> Escolher</button>
              </div>
              <div className="compare-folder-row">
                <div className="compare-folder-text">
                  <strong>Pasta candidata · conferir</strong>
                  <small title={candidateRoot}>{candidateRoot || 'Selecione a pasta com possíveis cópias'}</small>
                </div>
                <button type="button" className="outline-button" disabled={compareBusy || busy || searchBusy}
                  onClick={() => void pickComparisonFolder('candidate')}><FolderOpen size={16}/> Escolher</button>
              </div>
            </div>
            <button type="button" className="primary-button"
              disabled={!referenceRoot || !candidateRoot || compareBusy || busy || searchBusy}
              onClick={() => void compareFolders()}>
              {compareBusy ? <LoaderCircle className="spin" size={17}/> : <Fingerprint size={17}/>}
              {compareBusy ? 'Comparando…' : 'Comparar com BLAKE3'}
            </button>
            <p className="panel-note"><LockKeyhole size={14}/> Sem exclusão automática.
              Limites: 250 mil arquivos por pasta, 8 GiB de leituras de hash por comparação e no máximo 300 resultados exibidos.
              Pastas sobrepostas, arquivos em nuvem e links redirecionados são recusados ou ignorados.</p>
          </section>
          {comparison && <>
            <div className="metrics-grid">
              <Metric label="Candidatos correspondentes" value={number(comparison.matchedCandidates)}
                helper="Mesmo tamanho e BLAKE3, identidade independente" icon={Fingerprint} tone="mint"/>
              <Metric label="Economia lógica potencial" value={bytes(comparison.potentialLogicalSavingsBytes)}
                helper="Não equivale a espaço físico liberado" icon={HardDrive}/>
              <Metric label="Arquivos na pasta mestre" value={number(comparison.referenceFiles)}
                helper="Somente leitura" icon={FolderOpen} tone="violet"/>
              <Metric label="Leituras de hash" value={bytes(comparison.hashBytesRead)}
                helper="Limite de até 8 GiB" icon={Gauge} tone="pink"/>
            </div>
            {!comparison.complete && <div className="alert warning-alert" role="status">
              <Info size={18}/><span>A comparação é parcial.
                {comparison.truncated ? ' Limite de arquivos atingido.' : ''}
                {comparison.errors > 0 ? ' Erros de leitura ou de identidade: ' + number(comparison.errors) + '.' : ''}
                {comparison.skippedCloudFiles > 0 ?
                  ' Entradas em nuvem, links ou redirecionamentos ignorados: ' + number(comparison.skippedCloudFiles) + '.' : ''}
                Resultados ausentes não provam que um arquivo é único.</span>
            </div>}
            {comparison.hardlinkAliases > 0 && <p className="panel-note">
              <Info size={14}/> {number(comparison.hardlinkAliases)} hardlinks/aliases ignorados para evitar contar espaço duas vezes.
            </p>}
            <section className="panel glass">
              <SectionHeading kicker="REVISÃO MANUAL" title={number(comparison.matchedCandidates) + ' candidatos confirmados'}
                description="Compare os caminhos; nenhuma ação de exclusão está disponível aqui."
                right={<Tag tone={comparison.complete ? 'green' : 'amber'}>
                  {comparison.complete ? 'ANÁLISE CONCLUÍDA' : 'PARCIAL'}</Tag>}/>
              {comparison.matches.length === 0 && <div className="empty-list">
                Nenhuma cópia independente confirmada dentro dos limites da análise.
              </div>}
              {comparison.matches.map(item => <div className="compare-result" key={item.candidatePath}>
                <div className="compare-result-top"><strong>{bytes(item.sizeBytes)}</strong>
                  <small>BLAKE3 {item.hash.slice(0, 16)}…</small></div>
                <div className="compare-result-path"><span>Candidata</span>
                  <code title={item.candidatePath}>{item.candidatePath}</code>
                  <button type="button" className="icon-button" title="Copiar candidato"
                    aria-label="Copiar caminho do candidato" onClick={() => void copy(item.candidatePath)}>
                    <Clipboard size={15}/></button></div>
                <div className="compare-result-path"><span>Mestre</span>
                  <code title={item.referencePath}>{item.referencePath}</code>
                  <button type="button" className="icon-button" title="Copiar referência"
                    aria-label="Copiar caminho da referência" onClick={() => void copy(item.referencePath)}>
                    <Clipboard size={15}/></button></div>
              </div>)}
              {comparison.matchedCandidates > comparison.matches.length && <p className="panel-note">
                Exibindo os primeiros 300 caminhos confirmados; o contador considera todos os candidatos encontrados.
              </p>}
              <p className="panel-note">Relatório instantâneo de arquivos locais. Eles podem mudar após a análise.
                Antes de qualquer limpeza, faça backup e confirme novamente a identidade do arquivo.</p>
            </section>
          </>}
        </div>}

        {section === 'search' && report && <div className="stack-gap">
          <section className="panel glass search-panel">
            <SectionHeading kicker="BUSCA AVANÇADA" title="Pesquisa por Regex + tamanho" description="Filtre todo o escopo escolhido pelo nome/caminho, não apenas os arquivos visíveis."/>
            <form className="search-form" onSubmit={(event: FormEvent) => { event.preventDefault(); void searchMetadata(); }}>
              <label className="field"><span>Expressão regular (nome ou caminho)</span><div className="input-wrap"><Search size={19}/><input type="text" placeholder="Ex.: \\.(iso|zip|mp4)$" value={regex} onChange={(e) => setRegex(e.target.value)} spellCheck={false}/></div></label>
              <label className="field min-field"><span>Tamanho mínimo (MB)</span><div className="input-wrap"><SlidersHorizontal size={18}/><input type="number" min="0" step="1" value={minMb} onChange={(e) => setMinMb(e.target.value)}/></div></label>
              <button className="primary-button" type="submit" disabled={searchBusy || busy}>{searchBusy ? <LoaderCircle className="spin" size={17}/> : <Search size={17}/>} {searchBusy ? 'Buscando…' : 'Buscar'}</button>
            </form>
            <p className="panel-note"><Database size={14}/> Indexação sem teto artificial de arquivos.
              Registros são gravados em lotes de até 1.024; o snapshot anterior continua
              disponível até a atualização terminar. Cancelar não publica índices parciais.
              Pausar/Retomar mantém a tarefa ativa sem publicar dados parciais.
              Após fechar o aplicativo, é necessária nova enumeração para validar mudanças.</p>
            <div className="index-tools">
              <button type="button" className="outline-button" disabled={indexBusy || busy || searchBusy}
                onClick={() => void refreshIndex()}>
                {indexBusy ? <LoaderCircle className="spin" size={16}/> : <Database size={16}/>}
                {indexBusy ? 'Indexando…' : 'Atualizar índice SQLite'}
              </button>
              <label className="index-checkbox"><input type="checkbox"
                checked={useCachedIndex} onChange={event => setUseCachedIndex(event.target.checked)}/>
                Pesquisar último snapshot (sem acessar arquivos)
              </label>
            </div>
            {indexStats && <p className="panel-note">Snapshot: {new Date(indexStats.completedAtUnix * 1000).toLocaleString('pt-BR')} ·
              {number(indexStats.files)} arquivos · {number(indexStats.added)} novos ·
              {number(indexStats.changed)} alterados · {number(indexStats.removed)} removidos do índice · {number(indexStats.batchesWritten)} lotes SQLite gravados.
              {indexStats.skippedDirectories > 0 ? ' Diretórios redirecionados excluídos: ' + number(indexStats.skippedDirectories) + '.' : ''}
            </p>}
            {cachedAt && <p className="panel-note">Resultados em cache de {new Date(cachedAt * 1000).toLocaleString('pt-BR')};
              podem estar desatualizados até nova indexação.</p>}
            <p className="panel-note"><LockKeyhole size={14}/> Busca somente por metadados: não abre conteúdo, não recalcula hashes e preserva o relatório de duplicados. Regex usa a sintaxe do Rust regex.</p>
          </section>
          {searchResult && (searchResult.truncated || searchResult.errors > 0) && <div className="alert warning-alert"><Info size={18}/><span>Busca parcial: {searchResult.truncated ? 'limite explícito de amostragem atingido. ' : ''}{searchResult.errors > 0 ? number(searchResult.errors) + ' entradas inacessíveis.' : ''}</span></div>}
          <section className="panel glass"><SectionHeading kicker="RESULTADOS DE PESQUISA" title={number(searchResult?.totalMatches ?? report.totalMatches) + ' arquivos encontrados'} description="Exibindo até 500 resultados, em ordem decrescente de tamanho."/><FileRows files={searchResult?.matches ?? report.matches} copy={copy}/></section>
        </div>}

        {section === 'cleanup' && <div className="stack-gap">
          <div className="insight-banner glass">
            <div className="insight-icon"><ShieldCheck size={25}/></div>
            <div><small>QUARENTENA REVERSÍVEL · WINDOWS</small>
              <strong>Nenhuma exclusão permanente está disponível.</strong>
              <p>Escolha um único arquivo local, revise a pré-visualização e digite a confirmação.
                A movimentação somente funciona no mesmo volume do armazenamento local da aplicação.
                A quarentena não libera espaço em disco e não substitui backup.</p>
            </div>
          </div>
          <section className="panel glass">
            <SectionHeading kicker="PRÉ-VISUALIZAÇÃO OBRIGATÓRIA" title="Mover arquivo para quarentena"
              description="Pastas, OneDrive, arquivos do sistema, hardlinks e links simbólicos são bloqueados pelo backend."/>
            <div className="search-form">
              <label className="field"><span>Caminho absoluto do arquivo</span>
                <input type="text" value={quarantinePath} disabled={quarantineBusy}
                  onChange={event => { setQuarantinePath(event.target.value); setPreview(null); setMoveConfirmation(''); }}
                  placeholder="C:\\Users\\...\\arquivo.tmp"/></label>
              <button type="button" className="outline-button" disabled={quarantineBusy}
                onClick={() => void pickQuarantineFile()}>Selecionar arquivo</button>
              <button type="button" className="primary-button" disabled={!quarantinePath || quarantineBusy}
                onClick={() => void previewQuarantine()}>Pré-visualizar</button>
            </div>
            {preview && <div className="quarantine-preview" role="group" aria-label="Pré-visualização da quarentena">
              <strong title={preview.path}>{preview.path}</strong>
              <p>{bytes(preview.sizeBytes)} · {preview.warning}</p>
              <label className="field"><span>Digite MOVER PARA QUARENTENA para confirmar</span>
                <input type="text" autoComplete="off" value={moveConfirmation}
                  onChange={event => setMoveConfirmation(event.target.value)} /></label>
              <button type="button" className="primary-button"
                disabled={quarantineBusy || moveConfirmation !== 'MOVER PARA QUARENTENA'}
                onClick={() => void moveToQuarantine()}>Mover para quarentena</button>
            </div>}
          </section>
          <section className="panel glass">
            <SectionHeading kicker="RESTAURAÇÃO" title="Arquivos em quarentena"
              description="A restauração nunca substitui arquivos existentes no caminho original."
              right={<button type="button" className="outline-button"
                onClick={() => void loadQuarantine()}>Atualizar lista</button>}/>
            {quarantined.length > 0 && <label className="field">
              <span>Digite RESTAURAR para habilitar a restauração de um item</span>
              <input type="text" value={restoreConfirmation} autoComplete="off"
                onChange={event => setRestoreConfirmation(event.target.value)}/>
            </label>}
            {quarantined.map(item => <div className="quarantine-item" key={item.id}>
              <div><strong title={item.originalPath}>{truncatePath(item.originalPath, 100)}</strong>
                <small>{bytes(item.sizeBytes)} · {new Date(item.createdAtUnix * 1000).toLocaleString('pt-BR')}</small>
              </div>
              <button type="button" className="outline-button"
                disabled={quarantineBusy || restoreConfirmation !== 'RESTAURAR'}
                onClick={() => void restoreQuarantine(item.id)}>
                <ArchiveRestore size={16}/> Restaurar
              </button>
            </div>)}
            {!quarantined.length && <p className="panel-note">Nenhum arquivo recuperável listado neste computador.</p>}
            <p className="panel-note">Não há purga, retenção automática ou liberação física de espaço nesta versão.</p>
          </section>
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
          <section className="panel glass optimize-panel"><SectionHeading kicker="ADAPTADOR WINDOWS" title="Analisar ou otimizar unidade" description="Primeiro execute a análise. A autorização do backend expira em 5 minutos, vale somente para a mesma unidade e permite uma única otimização. Pode exigir privilégios de administrador."/>
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
