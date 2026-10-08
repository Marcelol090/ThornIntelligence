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
  maxFiles: number;
}

export interface SearchRequest {
  root: string;
  regex: string | null;
  minSizeBytes: number;
  maxFiles: number;
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

export type Section = 'overview' | 'explorer' | 'duplicates' | 'search' | 'optimize';

/** Lightweight IPC progress; no per-file paths or file contents are sent. */
export interface ScanProgress {
  phase: 'scanning' | 'hashing' | 'verifying' | 'searching' | 'complete';
  filesScanned: number;
  hashBytesRead: number;
}
