import type { ReactNode } from "react";
import { Folder, History, MessageSquare, Settings2, Workflow } from "lucide-react";
import { pages, type PageId } from "../presentation";

// Shared Lucide family; license is bundled with the application's static assets.
function NavigationIcon({ page }: { page: PageId }) {
  const Icon = { workbench: MessageSquare, editor: Workflow, files: Folder, tasks: History, settings: Settings2 }[page];
  return <Icon className="nav-icon" size={17} strokeWidth={1.65} aria-hidden="true" focusable="false" />;
}

type Props = {
  page: PageId;
  onNavigate: (page: PageId) => void;
  workspace: string | null;
  onWorkspace: () => void;
  children: ReactNode;
  notices?: ReactNode;
  noticesExpanded?: boolean;
  status?: ReactNode;
  activity?: ReactNode;
};

export default function AppShell({
  page,
  onNavigate,
  workspace,
  onWorkspace,
  children,
  notices,
  noticesExpanded = false,
  status,
  activity,
}: Props) {
  const current = pages.find((item) => item.id === page)!;
  const displayPath = workspace
    ?.replace(/^\\\\\?\\UNC\\/i, "\\\\")
    .replace(/^\\\\\?\\/, "");
  const folderName = displayPath
    ?.replace(/[\\/]+$/, "")
    .split(/[\\/]/)
    .pop();
  return (
    <div className={`app-layout${page === "workbench" ? " app-layout-workbench" : ""}`}>
      <aside className="app-sidebar" aria-label="应用导航">
        <div className="brand">
          <span className="brand-symbol" aria-hidden="true">
            AG
          </span>
          <div>
            <strong>AudioGraph</strong>
          </div>
        </div>
        <nav aria-label="主导航">
          {pages.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-current={page === item.id ? "page" : undefined}
              className={page === item.id ? "nav-link active" : "nav-link"}
              onClick={() => onNavigate(item.id)}
            >
              <NavigationIcon page={item.id} />
              {item.label}
            </button>
          ))}
        </nav>
      </aside>
      <div className="app-content">
        <header className="topbar">
          <div>
            <h1>{current.label}</h1>
          </div>
          {status}
          <button
            className="workspace-switch"
            type="button"
            onClick={onWorkspace}
            title={displayPath ?? "选择音频所在目录"}
          >
            {workspace ? `工作区：${folderName || displayPath}` : "打开工作区"}
          </button>
        </header>
        <div className={`app-shell-notices${noticesExpanded ? " notices-expanded" : ""}`}>{notices}</div>
        <main id="page-content" aria-label={current.label}>
          {children}
        </main>
        {activity}
      </div>
    </div>
  );
}
