import { File, FileAudio, FileJson, FileText, Folder, FolderOpen } from "lucide-react";
import { browserFileKind, type BrowserEntry } from "../workspace-browser-model";

// One SVG family and one optical size. Icons describe file type, not executability.
export default function WorkspaceEntryIcon({ entry, expanded = false }: { entry: BrowserEntry; expanded?: boolean }) {
  const kind = browserFileKind(entry);
  const type = entry.kind === "directory" ? "folder" : kind === "audio" ? "audio" : entry.name.toLowerCase().endsWith(".json") ? "json" : kind === "text" ? "text" : "file";
  const Icon = type === "folder" ? expanded ? FolderOpen : Folder : type === "audio" ? FileAudio : type === "json" ? FileJson : type === "text" ? FileText : File;
  return <Icon className={`workspace-entry-icon workspace-entry-icon-${type}`} size={16} strokeWidth={1.65} aria-hidden="true" focusable="false" />;
}
