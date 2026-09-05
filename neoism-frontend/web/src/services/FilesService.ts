// JS-side bridge for `neoism-ui::services::FilesService`.
//
// The Rust trait is synchronous and returns `IoError::Pending(req_id)`
// for the web case so panels can re-run after a `ServiceReply` arrives.
// On the JS side we expose a Promise-based API; the wasm chrome host
// drives the trait by either awaiting these promises directly or by
// allocating a request id, firing the call, and resuming the panel
// when `ProtocolClient.onServiceReply` lands the payload.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  DirEntry,
  FilesClientMessage,
  FilesServerMessage,
  TreeEntry,
  FileLocationDescriptor,
} from "../workspace/types";

export type { DirEntry, TreeEntry };

export interface FilesService {
  browserLocations(): Promise<FileLocationDescriptor[]>;
  browserListDir(path: string): Promise<DirEntry[]>;
  browserStat(path: string): Promise<DirEntry>;
  browserReadFile(path: string): Promise<Uint8Array>;
  listDir(path: string): Promise<DirEntry[]>;
  stat(path: string): Promise<DirEntry>;
  readFile(path: string): Promise<Uint8Array>;
  writeFile(path: string, bytes: Uint8Array): Promise<void>;
  walkTree(path: string, maxDepth?: number | null): Promise<TreeEntry[]>;
  /// Create an empty file under `dir/name`. Parent dirs are created.
  /// Mirrors the desktop `Screen::create_file_tree_file` path.
  createFile(dir: string, name: string): Promise<string>;
  /// Create a directory at `dir/name`. Idempotent (create_dir_all
  /// semantics).
  createDir(dir: string, name: string): Promise<string>;
  /// Rename or move `from` to `to`. Both are workspace-relative.
  rename(from: string, to: string): Promise<void>;
  /// Delete a file or directory. Directories are removed recursively.
  delete(path: string): Promise<boolean>;
}

function expectVariant<K extends string>(
  reply: FilesServerMessage,
  tag: K,
): Extract<FilesServerMessage, Record<K, unknown>> {
  if (tag in reply) {
    return reply as Extract<FilesServerMessage, Record<K, unknown>>;
  }
  if ("Error" in reply) {
    throw new Error(`files service error: ${reply.Error.message}`);
  }
  throw new Error(`unexpected reply variant for ${tag}`);
}

export class DaemonFilesService implements FilesService {
  constructor(private readonly client: ProtocolClient) {}

  async browserLocations(): Promise<FileLocationDescriptor[]> {
    const reply = await this.client.requestFiles("ListBrowserLocations");
    return expectVariant(reply, "BrowserLocations").BrowserLocations.locations;
  }

  async browserListDir(path: string): Promise<DirEntry[]> {
    const reply = await this.client.requestFiles({ BrowserListDir: { path } });
    return expectVariant(reply, "DirListing").DirListing.entries;
  }

  async browserStat(path: string): Promise<DirEntry> {
    const reply = await this.client.requestFiles({ BrowserStat: { path } });
    return expectVariant(reply, "Stat").Stat.entry;
  }

  async browserReadFile(path: string): Promise<Uint8Array> {
    const reply = await this.client.requestFiles({ BrowserReadFile: { path } });
    return Uint8Array.from(expectVariant(reply, "FileContent").FileContent.bytes);
  }

  async listDir(path: string): Promise<DirEntry[]> {
    const msg: FilesClientMessage = { ListDir: { path } };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "DirListing").DirListing.entries;
  }

  async stat(path: string): Promise<DirEntry> {
    const msg: FilesClientMessage = { Stat: { path } };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "Stat").Stat.entry;
  }

  async readFile(path: string): Promise<Uint8Array> {
    const msg: FilesClientMessage = { ReadFile: { path } };
    const reply = await this.client.requestFiles(msg);
    const bytes = expectVariant(reply, "FileContent").FileContent.bytes;
    return Uint8Array.from(bytes);
  }

  async writeFile(path: string, bytes: Uint8Array): Promise<void> {
    const msg: FilesClientMessage = {
      WriteFile: { path, bytes: Array.from(bytes) },
    };
    const reply = await this.client.requestFiles(msg);
    expectVariant(reply, "FileWritten");
  }

  async walkTree(
    path: string,
    maxDepth: number | null = null,
  ): Promise<TreeEntry[]> {
    const msg: FilesClientMessage = {
      WalkTree: { path, max_depth: maxDepth },
    };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "TreeListing").TreeListing.entries;
  }

  async createFile(dir: string, name: string): Promise<string> {
    const msg: FilesClientMessage = { CreateFile: { dir, name } };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "FileCreated").FileCreated.path;
  }

  async createDir(dir: string, name: string): Promise<string> {
    const msg: FilesClientMessage = { CreateDir: { dir, name } };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "FileCreated").FileCreated.path;
  }

  async rename(from: string, to: string): Promise<void> {
    const msg: FilesClientMessage = { Rename: { from, to } };
    const reply = await this.client.requestFiles(msg);
    expectVariant(reply, "Renamed");
  }

  async delete(path: string): Promise<boolean> {
    const msg: FilesClientMessage = { Delete: { path } };
    const reply = await this.client.requestFiles(msg);
    return expectVariant(reply, "Deleted").Deleted.was_dir;
  }
}
