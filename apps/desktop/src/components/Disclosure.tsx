import { useEffect, useId, useState, type ReactNode } from "react";

// A real button replaces native details/summary. Keep children mounted so hiding
// an editor or a form does not discard unsaved input or restart its controller.
export default function Disclosure({
  label,
  children,
  className = "",
  open,
}: {
  label: string;
  children: ReactNode;
  className?: string;
  open?: boolean;
}) {
  const id = useId();
  const [expanded, setExpanded] = useState(open ?? false);
  // Used for validation errors / empty experiment setup: react to a changed
  // automatic-open condition without undoing a user's toggle on every render.
  useEffect(() => {
    if (open !== undefined) setExpanded(open);
  }, [open]);
  return (
    <section className={`disclosure ${className}`}>
      <button
        type="button"
        className="disclosure-toggle"
        aria-expanded={expanded}
        aria-controls={id}
        onClick={() => setExpanded((value) => !value)}
      >
        {expanded ? `收起${label}` : `查看${label}`}
      </button>
      <div id={id} className="disclosure-content" hidden={!expanded}>
        {children}
      </div>
    </section>
  );
}
