export interface FileResult {
  name: string;
  path: string;
  sizeBytes: number;
  extension: string;
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

export interface OptimizationResult {
  drive: string;
  executed: boolean;
  output: string;
}

export type Section = 'overview' | 'explorer' | 'duplicates' | 'search' | 'optimize';
