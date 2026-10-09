import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronDown, ChevronRight, Clipboard, File, Folder, LoaderCircle, RefreshCcw } from 'lucide-react';
import type { TreeCursor, TreeNode, TreePage, TreeRequest } from '../types';
import { bytes, number, truncatePath } from '../lib/format';

const PAGE_SIZE = 100;
const ROW_HEIGHT = 46;
const VIEWPORT_HEIGHT = 460;
const OVERSCAN = 6;

type FolderState = {
  nodes: TreeNode[];
  nextCursor: TreeCursor | null;
  loaded: boolean;
  loading: boolean;
  error: string | null;
};
type DisplayRow = {
  key: string;
  depth: number;
  kind: 'node' | 'more' | 'loading' | 'error' | 'empty';
  path: string;
  node?: TreeNode;
};

export function IndexedTree({ root, revision, refreshing, onRefresh, onCopy }: {
  root: string;
  revision: number;
  refreshing: boolean;
  onRefresh: () => void;
  onCopy: (path: string) => void;
}) {
  const [folders, setFolders] = useState<Record<string, FolderState>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set([root]));
  const [scrollTop, setScrollTop] = useState(0);
  const [snapshotDate, setSnapshotDate] = useState<number | null>(null);
  const generation = useRef<number | null>(null);
  const requestEpoch = useRef(0);
  const busyFolders = useRef(new Set<string>());
  const viewport = useRef<HTMLDivElement>(null);

  async function load(parent: string, more: boolean) {
    const epoch = requestEpoch.current;
    const key = epoch + ':' + parent;
    if (busyFolders.current.has(key)) return;
    const old = folders[parent];
    if (more && (!old?.nextCursor || old.loading)) return;
    busyFolders.current.add(key);
    setFolders(current => ({
      ...current,
      [parent]: {
        nodes: more ? current[parent]?.nodes ?? [] : [],
        nextCursor: more ? current[parent]?.nextCursor ?? null : null,
        loaded: Boolean(more && current[parent]?.loaded),
        loading: true,
        error: null,
      },
    }));
    try {
      const request: TreeRequest = {
        root,
        parentPath: parent,
        limit: PAGE_SIZE,
        after: more ? old?.nextCursor ?? null : null,
        generation: generation.current,
      };
      const page = await invoke<TreePage>('browse_index_tree', { request });
      if (requestEpoch.current !== epoch) return;
      if (generation.current === null) generation.current = page.generation;
      setSnapshotDate(page.completedAtUnix);
      setFolders(current => ({
        ...current,
        [parent]: {
          nodes: more ? [...(current[parent]?.nodes ?? []), ...page.nodes] : page.nodes,
          nextCursor: page.nextCursor,
          loaded: true,
          loading: false,
          error: null,
        },
      }));
    } catch (err) {
      if (requestEpoch.current !== epoch) return;
      setFolders(current => ({
        ...current,
        [parent]: {
          nodes: current[parent]?.nodes ?? [],
          nextCursor: current[parent]?.nextCursor ?? null,
          loaded: Boolean(current[parent]?.loaded),
          loading: false,
          error: String(err),
        },
      }));
    } finally {
      busyFolders.current.delete(key);
    }
  }

  useEffect(() => {
    requestEpoch.current += 1;
    busyFolders.current.clear();
    generation.current = null;
    setSnapshotDate(null);
    setExpanded(new Set([root]));
    setFolders({});
    setScrollTop(0);
    if (viewport.current) viewport.current.scrollTop = 0;
    void load(root, false);
    // Reload only when scope changes or index commits a fresh generation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, revision]);

  const rows = useMemo(() => {
    const output: DisplayRow[] = [];
    function visit(parent: string, depth: number) {
      const folder = folders[parent];
      if (!folder || (folder.loading && !folder.loaded)) {
        output.push({ key: 'loading:' + parent, kind: 'loading', depth, path: parent });
        return;
      }
      if (folder.error && !folder.loaded) {
        output.push({ key: 'error:' + parent, kind: 'error', depth, path: parent });
        return;
      }
      if (folder.loaded && folder.nodes.length === 0) {
        output.push({ key: 'empty:' + parent, kind: 'empty', depth, path: parent });
      }
      for (const node of folder.nodes) {
        output.push({ key: node.path, kind: 'node', depth, path: node.path, node });
        if (node.kind === 'directory' && expanded.has(node.path)) {
          visit(node.path, depth + 1);
        }
      }
      if (folder.error) {
        output.push({ key: 'error:' + parent, kind: 'error', depth, path: parent });
      } else if (folder.nextCursor) {
        output.push({ key: 'more:' + parent, kind: 'more', depth, path: parent });
      }
    }
    visit(root, 1);
    return output;
  }, [root, folders, expanded]);

  const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const end = Math.min(rows.length, Math.ceil((scrollTop + VIEWPORT_HEIGHT) / ROW_HEIGHT) + OVERSCAN);
  const virtualRows = rows.slice(start, end);

  function expand(node: TreeNode) {
    if (node.kind !== 'directory') {
      onCopy(node.path);
      return;
    }
    const currentlyExpanded = expanded.has(node.path);
    setExpanded(current => {
      const next = new Set(current);
      if (currentlyExpanded) next.delete(node.path); else next.add(node.path);
      return next;
    });
    if (!currentlyExpanded && !folders[node.path]?.loaded && !folders[node.path]?.loading) {
      void load(node.path, false);
    }
  }

  return <div className="indexed-tree">
    <div className="indexed-tree-header">
      <div>
        <strong>Árvore indexada por diretório</strong>
        <small title={root}>{truncatePath(root, 100)} · snapshot {snapshotDate
          ? new Date(snapshotDate * 1000).toLocaleString('pt-BR') : 'aguardando índice'}</small>
      </div>
      <button className="outline-button" type="button" disabled={refreshing}
        onClick={onRefresh} title="Reenumerar metadados locais no SQLite, sem abrir conteúdo">
        {refreshing ? <LoaderCircle className="spin" size={15}/> : <RefreshCcw size={15}/>}
        {refreshing ? 'Indexando…' : 'Atualizar índice'}
      </button>
    </div>
    <p className="panel-note">Expanda pastas para consultar somente os filhos no SQLite. Até {number(PAGE_SIZE)} itens por página,
      com renderização limitada à área visível. Valores são lógicos, podem incluir hardlinks e refletem o último snapshot concluído.</p>
    <div className="indexed-tree-window" ref={viewport} role="tree"
      aria-label="Árvore de pastas e arquivos indexados" tabIndex={0}
      onScroll={event => setScrollTop(event.currentTarget.scrollTop)}>
      <div className="indexed-tree-space" style={{ height: rows.length * ROW_HEIGHT }}>
        {virtualRows.map((row, j) => {
          const index = start + j;
          const node = row.node;
          const folder = folders[row.path];
          const depth = Math.min(row.depth, 24);
          const isDirectory = node?.kind === 'directory';
          const isExpanded = Boolean(node && expanded.has(node.path));
          const message = row.kind === 'loading' ? 'Carregando diretório…'
            : row.kind === 'empty' ? 'Pasta sem itens indexados'
            : row.kind === 'error' ? 'Índice indisponível: clique para tentar novamente'
            : 'Carregar mais 100 itens';
          return <button key={row.key} type="button"
            className={'indexed-tree-row ' + (isDirectory ? 'is-folder' : '') +
              (row.kind !== 'node' ? ' is-placeholder' : '')}
            role="treeitem" aria-level={row.depth} aria-expanded={isDirectory ? isExpanded : undefined}
            aria-label={node ? (isDirectory ? 'Pasta ' : 'Copiar caminho do arquivo ') + node.name : message}
            title={node?.path ?? (row.kind === 'error' ? folder?.error ?? '' : row.path)}
            style={{ top: index * ROW_HEIGHT, height: ROW_HEIGHT, paddingLeft: 12 + depth * 17 }}
            disabled={row.kind === 'loading' || row.kind === 'empty' || Boolean(folder?.loading && row.kind === 'more')}
            onClick={() => {
              if (node) expand(node);
              else if (row.kind === 'more') void load(row.path, true);
              else if (row.kind === 'error') void load(row.path,
                Boolean(folder?.loaded && folder?.nextCursor));
            }}>
            <span className="indexed-tree-identity">
              {isDirectory ? (isExpanded ? <ChevronDown size={14}/> : <ChevronRight size={14}/>)
                : <span className="indexed-tree-chevron"/>}
              {row.kind === 'loading' ? <LoaderCircle className="spin" size={15}/>
                : isDirectory ? <Folder size={16}/> : <File size={16}/>}
              <span className="indexed-tree-name">{node?.name ?? message}</span>
              {node?.contentStatus === 'offline' && <small>Remoto</small>}
              {node?.contentStatus === 'reparse' && <small>Virtual</small>}
            </span>
            {node && <span className="indexed-tree-size">
              {bytes(node.sizeBytes)}
              {isDirectory && <small>{number(node.files)} arquivos</small>}
            </span>}
            {node?.kind === 'file' && <Clipboard size={13}/>}
          </button>;
        })}
      </div>
    </div>
    <p className="panel-note">{number(rows.filter(r => r.kind === 'node').length)} itens expandidos em memória;
      somente {number(virtualRows.length)} linhas montadas. A pasta original não é modificada.</p>
  </div>;
}
