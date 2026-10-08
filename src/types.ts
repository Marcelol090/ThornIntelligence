export interface FileResult {
  name: string;
  path: string;
  sizeBytes: number;
  extension: string;
  contentStatus: 'local' | 'offline' | 'reparse';
}

export interface DirectoryResult {
  name: string;
  path: string;
  sizeBytes: number;
  files: number;
}

export interface FileType {
  extension: string;
  sizeBytes: number;
  files: number;
}

export interface DuplicateGroup {
  hash: string;
  sizeBytes: number;
  copies: string[];
  potentialSavingsBytes: number;
}

export interface ScanReport {
  root: string;
  filesScanned: number;
  directoriesScanned: number;
  logicalBytes: number;
  elapsedMs: number;
  errors: number;
  errorSamples: string[];
  truncated: boolean;
  duplicateAnalysisComplete: boolean;
  hashingSkipped: boolean;
  hardlinkAliases: number;
  skippedContentFiles: number;
  hashBytesRead: number;
  topFiles: FileResult[];
  topDirectories: DirectoryResult[];
  fileTypes: FileType[];
  duplicates: DuplicateGroup[];
  matches: FileResult[];
  totalMatches: number;
  potentialSavingsBytes: number;
}

export interface ScanRequest {
  root: string;
  regex: string | null;
  minSizeBytes: number;
  /** Omit to scan every accessible file; set only for explicit sampling. */
  maxFiles?: number;
  analyzeDuplicates?: boolean;
}

export interface SearchRequest {
  root: string;
  regex: string | null;
  minSizeBytes: number;
  /** Omit to scan every accessible file; set only for explicit sampling. */
  maxFiles?: number;
}

export interface SearchReport {
  root: string;
  filesScanned: number;
  totalMatches: number;
  matches: FileResult[];
  elapsedMs: number;
  errors: number;
  truncated: boolean;
}

export interface DiskHealth {
  drive: string;
  model: string | null;
  busType: string | null;
  diskNumber: number | null;
  healthStatus: string | null;
  operationalStatus: string | null;
  sizeBytes: number | null;
  freeBytes: number | null;
  temperatureC: number | null;
  wearPercent: number | null;
  readErrorsUncorrected: number | null;
  writeErrorsUncorrected: number | null;
  powerOnHours: number | null;
  reliabilityAvailable: boolean;
}

export interface OptimizationResult {
  drive: string;
  executed: boolean;
  output: string;
}

export type Section = 'overview' | 'explorer' | 'duplicates' | 'search' | 'optimize' | 'compare' | 'cleanup';

/** Lightweight IPC progress; no per-file paths or file contents are sent. */
export interface ScanProgress {
  phase: 'scanning' | 'fingerprinting' | 'hashing' | 'verifying' | 'searching' | 'comparing' | 'allocation' | 'indexing' | 'paused' | 'complete';
  filesScanned: number;
  hashBytesRead: number;
}

/** Read-only per-file Windows allocation query; never estimates recoverable space. */
export interface AllocationTarget {
  path: string;
  expectedSizeBytes: number;
}
export interface AllocationRequest {
  root: string;
  targets: AllocationTarget[];
}
export interface AllocationItem {
  path: string;
  logicalBytes: number;
  allocatedBytes: number | null;
  /** Volume-wide NTFS hardlink count; not confined to selected root. */
  hardlinkCount: number | null;
  status: 'measured' | 'excluded' | 'changed' | 'outside' | 'unavailable' | 'unsupported';
}
export interface AllocationReport {
  items: AllocationItem[];
  measured: number;
  skipped: number;
  failed: number;
  elapsedMs: number;
}

/** Comparison never deletes or moves files; its savings are logical estimates. */
export interface CompareRequest {
  referenceRoot: string;
  candidateRoot: string;
  maxFiles: number;
}
export interface CompareMatch {
  candidatePath: string;
  referencePath: string;
  sizeBytes: number;
  hash: string;
}
export interface CompareReport {
  referenceRoot: string;
  candidateRoot: string;
  referenceFiles: number;
  candidateFiles: number;
  matchedCandidates: number;
  matches: CompareMatch[];
  potentialLogicalSavingsBytes: number;
  hashBytesRead: number;
  skippedCloudFiles: number;
  hardlinkAliases: number;
  errors: number;
  errorSamples: string[];
  truncated: boolean;
  complete: boolean;
  elapsedMs: number;
}

export interface IndexStats {
  root: string;
  files: number;
  added: number;
  changed: number;
  unchanged: number;
  removed: number;
  skippedDirectories: number;
  completedAtUnix: number;
  elapsedMs: number;
  batchesWritten: number;
}
export interface IndexedSearch {
  report: SearchReport;
  completedAtUnix: number;
}

/** One keyset page from the last published SQLite directory snapshot. */
export interface TreeCursor {
  kind: 'directory' | 'file';
  sizeBytes: number;
  path: string;
}
export interface TreeRequest {
  root: string;
  parentPath: string;
  after?: TreeCursor | null;
  limit?: number;
  generation?: number | null;
}
export interface TreeNode {
  path: string;
  name: string;
  kind: 'directory' | 'file';
  sizeBytes: number;
  files: number;
  contentStatus: 'local' | 'offline' | 'reparse';
}
export interface TreePage {
  root: string;
  parentPath: string;
  nodes: TreeNode[];
  nextCursor: TreeCursor | null;
  completedAtUnix: number;
  generation: number;
}
export interface QuarantinePreview {
  previewId: string;
  path: string;
  sizeBytes: number;
  warning: string;
}
export interface QuarantineItem {
  id: string;
  originalPath: string;
  sizeBytes: number;
  createdAtUnix: number;
  status: string;
}
