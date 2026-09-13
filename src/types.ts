export interface SharedFile {
  id: number;
  name: string;
  path: string;
  size: number;
}

export interface FailedAdd {
  path: string;
  error: string;
}

export interface AddFilesOutcome {
  added: SharedFile[];
  failed: FailedAdd[];
}

export interface ServerInfo {
  running: boolean;
  url: string | null;
  port: number | null;
  lan_ip: string | null;
}
