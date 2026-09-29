import type { ReactNode } from "react";
import { pages, type PageId } from "../presentation";

type Props = {
  page: PageId;
  onNavigate: (page: PageId) => void;
  workspace: string | null;
  onWorkspace: () => void;
  children: ReactNode;
  notices?: ReactNode;
  activity?: ReactNode;
};

export default function AppShell({
  page,
  onNavigate,
  workspace,
  onWorkspace,
  children,
  notices,
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
    <div className="app-layout">
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
          <button
            className="workspace-switch"
            type="button"
            onClick={onWorkspace}
            title={displayPath ?? "选择音频所在目录"}
          >
            {workspace ? `工作区：${folderName || displayPath}` : "打开工作区"}
          </button>
        </header>
        {notices}
        <main id="page-content" aria-label={current.label}>
          {children}
        </main>
        {activity}
      </div>
    </div>
  );
}
