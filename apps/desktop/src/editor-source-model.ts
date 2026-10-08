export type EditorSource = {
  context: string;
  path: string;
  revision: string;
  content: string;
  space?: "user" | "ai";
};

export function sourceForContext(source: EditorSource | null, context: string) {
  return source?.context === context ? source : null;
}

export function editorModified(source: EditorSource | null, content: string) {
  return source === null || source.content !== content;
}

function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value !== null && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return Object.fromEntries(Object.keys(object).sort().map((key) => [key, canonicalJson(object[key])]));
  }
  return value;
}

/** Native JSON may reorder object fields; arrays and all values remain exact. */
export function sameGraphContent(left: unknown, right: unknown): boolean {
  return JSON.stringify(canonicalJson(left)) === JSON.stringify(canonicalJson(right));
}

function comparableWindowsPath(path: string): string {
  const slashes = path.replace(/\\/g, "/");
  return slashes.replace(/^\/\/\?\/UNC\//i, "//").replace(/^\/\/\?\//, "").replace(/\/$/, "");
}

export type EditorWorkspace = { workspace: string | null; scope: string };

export function editorWorkspaceContext(owner: EditorWorkspace): string {
  return JSON.stringify([owner.workspace, owner.scope]);
}

/** Connection tokens expire independently of a draft's remembered file owner. */
export function transitionEditorWorkspace(
  previous: EditorWorkspace,
  source: EditorSource | null,
  workspace: string | null,
  scope = "",
): { owner: EditorWorkspace; source: EditorSource | null } {
  const owner = {
    workspace: workspace === null ? previous.workspace : comparableWindowsPath(workspace).toLowerCase(),
    scope,
  };
  return {
    owner,
    source: editorWorkspaceContext(previous) === editorWorkspaceContext(owner) ? source : null,
  };
}

export function workspaceRelativeSavePath(workspace: string, selected: string): string {
  const root = comparableWindowsPath(workspace);
  const file = comparableWindowsPath(selected);
  if (!file.toLowerCase().startsWith(`${root.toLowerCase()}/`)) {
    throw new Error("请选择用户工作区内的新文件路径。");
  }
  return file.slice(root.length + 1);
}

/** Persist the captured content, so edits made while saving stay modified. */
export function savedEditorSource(
  context: string,
  currentContext: string,
  path: string,
  revision: string,
  content: string,
  space?: "user" | "ai",
): EditorSource | null {
  return context === currentContext
    ? { context, path, revision, content, space }
    : null;
}
